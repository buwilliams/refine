use super::*;

pub fn validate_promotion(snapshot: &SourcePromotionSnapshot) -> RefineResult<()> {
    if !snapshot.clean {
        return Err(RefineError::Conflict(
            "source promotion requires a clean controller checkout; dirty work was left untouched"
                .to_string(),
        ));
    }
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
    if !snapshot.active_work.is_empty() {
        return Err(RefineError::Conflict(format!(
            "source promotion requires an idle Refine runtime: {}",
            snapshot.active_work.join(", ")
        )));
    }
    Ok(())
}
