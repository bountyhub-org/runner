use std::sync::Arc;

use cel_interpreter::Value as CelValue;
use client::job::{TimelineRequestStepOutcome, TimelineRequestStepState};
use jobengine::JobEngine;
use uuid::Uuid;

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

        Self {
            workdir,
            envs,
            engine,
            cfg,
            ok: true,
        }
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
    use std::{collections::BTreeMap, sync::Arc};
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
            CelValue::String(Arc::new(job_config.scans["example"][0].id.to_string()))
        );
        assert_eq!(
            ctx.eval("project.id").unwrap(),
            CelValue::String(Arc::new(job_config.project.id.to_string()))
        );
        assert_eq!(
            ctx.eval("workflow.id").unwrap(),
            CelValue::String(Arc::new(job_config.workflow.id.to_string()))
        );
        assert_eq!(
            ctx.eval("revision.id").unwrap(),
            CelValue::String(Arc::new(job_config.revision.id.to_string()))
        );
        assert_eq!(
            ctx.eval("vars.key").unwrap(),
            CelValue::String(Arc::new("value".to_string()))
        );
        assert_eq!(
            ctx.eval("name").unwrap(),
            CelValue::String(Arc::new(job_config.name))
        );
        assert_eq!(
            ctx.eval("id").unwrap(),
            CelValue::String(Arc::new(job_config.id.to_string()))
        );
    }
}
