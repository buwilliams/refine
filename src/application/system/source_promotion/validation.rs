use super::*;

/// Validate that an update is genuinely available and reachable by fast-forward.
///
/// This intentionally ignores checkout cleanliness and runtime quiescence: it
/// gates whether an update *should begin at all*, before any normalization
/// (such as stashing dirty work) is allowed to touch the tree.
pub fn validate_update_intent(snapshot: &SourcePromotionSnapshot) -> RefineResult<()> {
    if !snapshot.fast_forward {
        return Err(RefineError::Conflict(
            "source promotion requires fast-forward-only ancestry; the checkout and remote diverged"
                .to_string(),
        ));
    }
    if !snapshot.update_available {
        return Err(RefineError::Conflict(
            "the running checkout is already at the latest fetched source commit".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_promotion(snapshot: &SourcePromotionSnapshot) -> RefineResult<()> {
    if !snapshot.clean {
        return Err(RefineError::Conflict(
            "source promotion requires a clean controller checkout; dirty work was left untouched"
                .to_string(),
        ));
    }
    validate_update_intent(snapshot)?;
    if !snapshot.active_work.is_empty() {
        return Err(RefineError::Conflict(format!(
            "source promotion requires an idle Refine runtime: {}",
            snapshot.active_work.join(", ")
        )));
    }
    Ok(())
}
