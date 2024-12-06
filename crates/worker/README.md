# Worker

Worker is the application responsible for actually executing the job. The worker:
- Resolves the job.
- Parses the job.
- Executes all steps.
- Reports the result, logs, timeline, etc. back to the server.
- Once the job is done, worker shuts down.
