//! Required binding completion and immutable commitments used at gate settlement.
use super::*;

/// Captured from preparation, independently of whether any results were returned.
/// Execution may append process metadata; it cannot replace these commitments.
#[derive(Clone)]
pub(crate) struct BlockingInvocation {
    pub(crate) id: String,
    event: EventDefinition,
    config_revision: u64,
    bindings: Vec<PinnedBinding>,
    context: InvocationContext,
}

impl BlockingInvocation {
    pub(crate) fn pin(invocation: &EventInvocation) -> Option<Self> {
        invocation
            .bindings
            .iter()
            .any(|p| p.binding.mode == BindingMode::Blocking)
            .then(|| {
                let mut context = invocation.context.clone();
                context.metadata.clear();
                Self {
                    id: invocation.id.clone(),
                    event: invocation.event.clone(),
                    config_revision: invocation.config_revision,
                    bindings: invocation.bindings.clone(),
                    context,
                }
            })
    }

    pub(crate) fn validate_completion(
        &self,
        service: &FileEventService,
    ) -> RefineResult<EventInvocation> {
        let invocation = service.invocation(&self.id)?;
        let mut context = invocation.context.clone();
        context.metadata.clear();
        if invocation.id != self.id
            || invocation.event != self.event
            || invocation.config_revision != self.config_revision
            || invocation.bindings != self.bindings
            || context != self.context
        {
            return Err(RefineError::Conflict(format!(
                "Skill invocation {} binding, authority or original workspace commitment changed before settlement",
                self.id
            )));
        }
        invocation.blocking_results()?;
        service.validate_lifecycle(&invocation)?;
        invocation.context.validate_workspace(&service.refine_dir)?;
        Ok(invocation)
    }

    pub(crate) fn validate(&self, service: &FileEventService) -> RefineResult<()> {
        let invocation = self.validate_completion(service)?;
        let results = invocation.blocking_results()?;
        if let Some(result) = results.iter().find(|r| r.outcome != "success") {
            return Err(RefineError::Degraded(format!(
                "Skill invocation {} binding {} failed: {}",
                self.id, result.binding_id, result.summary
            )));
        }
        Ok(())
    }
}

impl FileEventService {
    /// Caller holds the Goal lock. Lock required records in stable order only for
    /// validation and settlement, so cancellation cannot slip between them.
    pub(crate) fn settle_blocking<T>(
        &self,
        required: &[BlockingInvocation],
        settle: impl FnOnce(RefineResult<()>) -> RefineResult<T>,
    ) -> RefineResult<T> {
        fn locked<T>(
            service: &FileEventService,
            required: &[&BlockingInvocation],
            all: &[&BlockingInvocation],
            settle: impl FnOnce(RefineResult<()>) -> RefineResult<T>,
        ) -> RefineResult<T> {
            match required.split_first() {
                Some((first, remaining)) => {
                    with_record_lock(&service.refine_dir, &format!("event-{}", first.id), || {
                        locked(service, remaining, all, settle)
                    })
                }
                None => settle(
                    all.iter()
                        .try_for_each(|required| required.validate(service)),
                ),
            }
        }
        let mut ordered: Vec<_> = required.iter().collect();
        ordered.sort_by(|a, b| a.id.cmp(&b.id));
        locked(self, &ordered, &ordered, settle)
    }
}
