use super::*;

impl LocalDaemonWebServer for LocalHttpDaemon {
    fn serve(&self, port: u16) -> RefineResult<DaemonStatus> {
        self.recover_runtime_state()?;
        let listener = Self::bind_loopback(port)?;
        let addr = Self::local_addr(&listener)?;
        let status = self.server.status.clone();
        self.serve_listener(listener, None)?;
        let mut status = status;
        status.port = addr.port();
        Ok(status)
    }

    fn server_sent_events(&self, stream: &str) -> RefineResult<String> {
        let mut events = String::new();
        events.push_str("retry: 3000\n");
        for frame in self.server_sent_event_frames(stream)? {
            events.push_str(&sse_event(frame.event, frame.data)?);
        }
        Ok(events)
    }
}
