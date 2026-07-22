Derive concise lifecycle instructions from the target app itself. Probe blind spots instead of guessing. Return only JSON with kind=target-app and fields start_instructions, stop_instructions, build_instructions, test_command, status_command, cwd, env, start_timeout_seconds, stop_timeout_seconds, build_timeout_seconds, test_timeout_seconds, status_timeout_seconds, log_path, http_check_url, tcp_check_host, tcp_check_port, process_check_command, and notes. Start, stop, and build are agent instructions; test and status may be commands.

Project root: {{target_root}}
