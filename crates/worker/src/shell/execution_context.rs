use cellang::Value as CelValue;
use client::job::{TimelineRequestStepOutcome, TimelineRequestStepState};
use jobengine::JobEngine;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug)]
pub struct ExecutionContext {
    workdir: String,
    envs: Arc<Vec<(String, String)>>,
    cfg: jobengine::Config,
    engine: JobEngine,
    ok: bool,
}

impl ExecutionContext {
    pub fn new(workdir: String, envs: Arc<Vec<(String, String)>>, cfg: jobengine::Config) -> Self {
        let engine = JobEngine::new(&cfg);

        let mut envs = envs.iter().cloned().collect::<Vec<(String, String)>>();
        envs.push((
            "BOUNTYHUB_PROJECT_ID".to_string(),
            cfg.project.id.to_string(),
        ));
        envs.push((
            "BOUNTYHUB_WORKFLOW_ID".to_string(),
            cfg.workflow.id.to_string(),
        ));
        envs.push((
            "BOUNTYHUB_REVISION_ID".to_string(),
            cfg.revision.id.to_string(),
        ));
        envs.push(("BOUNTYHUB_JOB_ID".to_string(), cfg.id.to_string()));
        envs.push(("BOUNTYHUB_JOB_NAME".to_string(), cfg.name.clone()));

        Self {
            workdir,
            envs: Arc::new(envs),
            engine,
            cfg,
            ok: true,
        }
    }

    #[inline]
    pub fn ok(&self) -> bool {
        self.ok
    }

    #[inline]
    pub fn workdir(&self) -> &str {
        &self.workdir
    }

    #[inline]
    pub fn envs(&self) -> Arc<Vec<(String, String)>> {
        self.envs.clone()
    }

    #[tracing::instrument(skip(self))]
    pub fn eval(&self, expr: &str) -> Result<CelValue, String> {
        self.engine.eval(expr).map_err(|err| format!("{:?}", err))
    }

    #[inline]
    pub fn project_id(&self) -> Uuid {
        self.cfg.project.id
    }

    #[inline]
    pub fn workflow_id(&self) -> Uuid {
        self.cfg.workflow.id
    }

    #[inline]
    pub fn revision_id(&self) -> Uuid {
        self.cfg.revision.id
    }

    #[inline]
    pub fn job_id(&self) -> Uuid {
        self.cfg.id
    }

    /// Update the execution context with the state of the step.
    /// If success is already false, it will not be updated.
    #[tracing::instrument(skip(self))]
    pub fn update_state(&mut self, state: TimelineRequestStepState) {
        if !self.ok {
            return;
        }
        let fail = match state {
            TimelineRequestStepState::Cancelled => true,
            TimelineRequestStepState::Failed { outcome, .. } => {
                matches!(outcome, TimelineRequestStepOutcome::Failed)
            }
            _ => false,
        };
        self.ok = !fail;
        self.engine.set_ok(self.ok);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::job::TimelineRequestStepState;
    use jobengine::{JobMeta, ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
    use std::{collections::BTreeMap, env, sync::Arc};
    use uuid::Uuid;

    #[test]
    fn test_update_state() {
        let mut ctx = super::ExecutionContext::new(
            "workdir".to_string(),
            Arc::new(vec![("key".to_string(), "value".to_string())]),
            jobengine::Config {
                id: Uuid::now_v7(),
                name: "example".to_string(),
                scans: BTreeMap::new(),
                inputs: None,
                project: ProjectMeta { id: Uuid::now_v7() },
                workflow: WorkflowMeta { id: Uuid::now_v7() },
                revision: WorkflowRevisionMeta { id: Uuid::now_v7() },
                vars: BTreeMap::new(),
            },
        );
        assert!(ctx.ok);

        ctx.update_state(TimelineRequestStepState::Running);
        assert!(ctx.ok);

        ctx.update_state(TimelineRequestStepState::Succeeded);
        assert!(ctx.ok);

        ctx.update_state(TimelineRequestStepState::Skipped);
        assert!(ctx.ok);

        ctx.update_state(TimelineRequestStepState::Cancelled);
        assert!(!ctx.ok);

        ctx.update_state(TimelineRequestStepState::Succeeded);
        assert!(!ctx.ok);

        ctx.ok = true;
        ctx.update_state(TimelineRequestStepState::Failed {
            outcome: TimelineRequestStepOutcome::Succeeded,
        });
        assert!(ctx.ok);

        ctx.update_state(TimelineRequestStepState::Failed {
            outcome: TimelineRequestStepOutcome::Failed,
        });
        assert!(!ctx.ok);

        ctx.update_state(TimelineRequestStepState::Succeeded);
        assert!(!ctx.ok);
    }

    #[test]
    fn test_eval() {
        let job_config = jobengine::Config {
            id: Uuid::now_v7(),
            name: "example".to_string(),
            scans: {
                let mut m = BTreeMap::new();
                m.insert(
                    "example".to_string(),
                    vec![JobMeta {
                        id: Uuid::now_v7(),
                        state: "succeeded".to_string(),
                        nonce: Some("nonce:example".to_string()),
                    }],
                );
                m
            },
            inputs: None,
            project: ProjectMeta { id: Uuid::now_v7() },
            workflow: WorkflowMeta { id: Uuid::now_v7() },
            revision: WorkflowRevisionMeta { id: Uuid::now_v7() },
            vars: {
                let mut m = BTreeMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
        };
        let ctx = super::ExecutionContext::new(
            "workdir".to_string(),
            Arc::new(vec![("key".to_string(), "value".to_string())]),
            job_config.clone(),
        );

        assert_eq!(ctx.eval("1 + 1").unwrap(), CelValue::Int(2));
        assert_eq!(
            ctx.eval("scans.example[0].id").unwrap(),
            CelValue::String(job_config.scans["example"][0].id.to_string())
        );
        assert_eq!(
            ctx.eval("project.id").unwrap(),
            CelValue::String(job_config.project.id.to_string())
        );
        assert_eq!(
            ctx.eval("workflow.id").unwrap(),
            CelValue::String(job_config.workflow.id.to_string())
        );
        assert_eq!(
            ctx.eval("revision.id").unwrap(),
            CelValue::String(job_config.revision.id.to_string())
        );
        assert_eq!(
            ctx.eval("vars.key").unwrap(),
            CelValue::String("value".to_string())
        );
        assert_eq!(ctx.eval("name").unwrap(), CelValue::String(job_config.name));
        assert_eq!(
            ctx.eval("id").unwrap(),
            CelValue::String(job_config.id.to_string())
        );
    }

    #[test]
    fn test_environment_is_set() {
        let project_id = Uuid::now_v7();
        let workflow_id = Uuid::now_v7();
        let revision_id = Uuid::now_v7();
        let job_id = Uuid::now_v7();

        let ctx = super::ExecutionContext::new(
            "workdir".to_string(),
            Arc::new(vec![("key".to_string(), "value".to_string())]),
            jobengine::Config {
                id: job_id,
                name: "example".to_string(),
                scans: BTreeMap::new(),
                inputs: None,
                project: ProjectMeta { id: project_id },
                workflow: WorkflowMeta { id: workflow_id },
                revision: WorkflowRevisionMeta { id: revision_id },
                vars: BTreeMap::new(),
            },
        );

        let envs = ctx.envs();
        let got = envs.iter().cloned().collect::<BTreeMap<String, String>>();
        assert_eq!(
            got.get("BOUNTYHUB_JOB_ID")
                .expect("BOUNTYHUB_JOB_ID")
                .clone(),
            job_id.to_string()
        );
        assert_eq!(
            got.get("BOUNTYHUB_JOB_NAME")
                .expect("BOUNTYHUB_JOB_NAME")
                .clone(),
            "example".to_string(),
        );
        assert_eq!(
            got.get("BOUNTYHUB_PROJECT_ID")
                .expect("BOUNTYHUB_PROJECT_ID")
                .clone(),
            project_id.to_string(),
        );
        assert_eq!(
            got.get("BOUNTYHUB_WORKFLOW_ID")
                .expect("BOUNTYHUB_WORKFLOW_ID")
                .clone(),
            workflow_id.to_string(),
        );
        assert_eq!(
            got.get("BOUNTYHUB_REVISION_ID")
                .expect("BOUNTYHUB_REVISION_ID")
                .clone(),
            revision_id.to_string(),
        );
    }
}
