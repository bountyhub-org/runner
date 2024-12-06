# Runner

Runner is crate that polls for jobs and hand them over to workers. It uses the main invoker API to:
- Cancel jobs during startup. Every job running on this runner will be canceled.
- Cancel jobs during shutdown. Every job running on this runner will be canceled.
- Poll for new jobs. Uses long-poll mechanism to communicate the runner capacity, and hands over available jobs to workers.
