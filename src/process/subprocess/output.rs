use super::*;

impl FileProcessSupervisor {
    pub fn recent_output_observations(
        &self,
        limit: usize,
    ) -> RefineResult<Vec<ProcessOutputObservation>> {
        let mut selected = self
            .list()?
            .into_iter()
            .rev()
            .take(limit)
            .collect::<Vec<_>>();
        selected.reverse();
        #[cfg(test)]
        run_after_process_enumeration_hook(&self.runtime_root);
        selected
            .iter()
            .map(|process| self.observe_output(process))
            .collect()
    }
}
