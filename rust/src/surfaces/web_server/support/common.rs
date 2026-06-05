use chrono::Utc;

pub(in crate::surfaces::web_server) fn now_timestamp_web() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
