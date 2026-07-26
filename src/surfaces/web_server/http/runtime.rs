use super::*;

impl LocalHttpDaemon {
    pub fn recover_runtime_state(&self) -> RefineResult<()> {
        self.recover_runtime_state_with_progress(|_| {})
    }

    pub fn recover_runtime_state_with_progress(
        &self,
        mut report: impl FnMut(&str),
    ) -> RefineResult<()> {
        if let Some(runtime_root) = &self.server.runtime_root {
            report("recovering supervised operations");
            FileOperationRegistry::new(runtime_root).recover_active_supervised()?;
            report("recovering incomplete Quality cancellations");
            QualityOperationRunner::recover_cancelled_operations_for_runtime(runtime_root)?;
        }
        if let Some(refine_dir) = self.server.current_refine_dir()? {
            report("recovering interrupted chat turns");
            self.server
                .chat_service(&refine_dir)
                .recover_interrupted_turns(
                    "Daemon restarted before the provider turn completed.",
                )?;
        }
        report("warming project and runtime caches");
        let _ = self.server.warm_current_projection_cache()?;
        report("warming diagnostics cache");
        self.server.warm_diagnostics_cache()?;
        report("warming static asset cache");
        self.warm_static_cache()?;
        report("startup cache warming complete");
        Ok(())
    }

    pub(super) fn warm_static_cache(&self) -> RefineResult<()> {
        let Some(static_root) = &self.static_root else {
            return Ok(());
        };
        warm_static_cache_dir(static_root, static_root)
    }

    pub fn bind_loopback(port: u16) -> RefineResult<TcpListener> {
        Self::bind_address(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    pub fn bind_address(bind_address: IpAddr, port: u16) -> RefineResult<TcpListener> {
        TcpListener::bind((bind_address, port)).map_err(|error| {
            RefineError::Io(format!(
                "failed to bind local daemon web server on {bind_address}:{port}: {error}"
            ))
        })
    }

    pub fn local_addr(listener: &TcpListener) -> RefineResult<SocketAddr> {
        listener.local_addr().map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect daemon listener address: {error}"
            ))
        })
    }

    pub fn serve_once(&self, listener: TcpListener) -> RefineResult<()> {
        self.serve_listener(listener, Some(serve_once_shutdown()))
    }

    pub fn serve_static(&self, listener: TcpListener) -> RefineResult<()> {
        self.serve_listener(listener, None)
    }

    pub fn serve_until_unhealthy(
        &self,
        listener: TcpListener,
        lifecycle: FileDaemonLifecycleService,
        port: u16,
    ) -> RefineResult<()> {
        let _automation_loop = if agent_automation_loop_disabled() {
            None
        } else {
            Some(self.start_agent_automation_loop(AGENT_WORKFLOW_INTERVAL))
        };
        let _git_sync_loop = self.start_git_sync_loop();
        self.serve_listener(listener, Some(lifecycle_shutdown(lifecycle, port)))
    }
}
