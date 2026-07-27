use super::*;

#[test]
fn agent_configure_and_diagnose_use_provider_service() {
    assert!(Cli::try_parse_from(["refine", "agent", "supervisor"]).is_err());
    dispatch(
        Cli::try_parse_from(["refine", "agent", "configure", "--provider", "smoke-ai"]).unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from(["refine", "agent", "diagnose", "--provider", "smoke-ai"]).unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "agent",
            "configure",
            "--provider",
            "configured-generic-agent",
        ])
        .unwrap(),
    )
    .unwrap();
}
