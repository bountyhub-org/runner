use cellang::Value as CelValue;
use jobengine::JobEngine;
use miette::Result;
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

#[derive(Debug)]
pub struct ExecutionContext {
    workdir: PathBuf,
    job_dir: PathBuf,
    env: Arc<Vec<(String, String)>>,
    cfg: Arc<jobengine::Config>,
    engine: JobEngine,
    ok: bool,
}

impl ExecutionContext {
    #[tracing::instrument]
    pub fn new(workdir: PathBuf, env: Arc<Vec<(String, String)>>, cfg: jobengine::Config) -> Self {
        let engine = JobEngine::new(&cfg);

        let mut env = env.iter().cloned().collect::<Vec<(String, String)>>();
        env.push((
            "BOUNTYHUB_PROJECT_ID".to_string(),
            cfg.project.id.to_string(),
        ));
        env.push((
            "BOUNTYHUB_WORKFLOW_ID".to_string(),
            cfg.workflow.id.to_string(),
        ));
        env.push((
            "BOUNTYHUB_REVISION_ID".to_string(),
            cfg.revision.id.to_string(),
        ));
        env.push(("BOUNTYHUB_JOB_ID".to_string(), cfg.id.to_string()));
        env.push(("BOUNTYHUB_SCAN_NAME".to_string(), cfg.name.clone()));

        for (k, v) in &cfg.env {
            match engine.eval_templ(v) {
                Ok(v) => env.push((k.clone(), v)),
                Err(e) => {
                    tracing::warn!("Failed to parse environment variable {k}: {e:?}. Continuing")
                }
            };
        }

        let job_dir = PathBuf::from(&workdir).join(cfg.id.to_string());
        Self {
            workdir,
            job_dir,
            env: Arc::new(env),
            engine,
            cfg: Arc::new(cfg),
            ok: true,
        }
    }

    #[inline]
    pub fn ok(&self) -> bool {
        self.ok
    }

    #[inline]
    pub fn workdir(&self) -> &PathBuf {
        &self.workdir
    }

    #[inline]
    pub fn job_dir(&self) -> &PathBuf {
        &self.job_dir
    }

    #[inline]
    pub fn env(&self) -> Arc<Vec<(String, String)>> {
        self.env.clone()
    }

    #[tracing::instrument(skip(self))]
    pub fn eval_expr(&self, expr: &str) -> Result<CelValue> {
        self.engine.eval_expr(expr)
    }

    #[tracing::instrument(skip(self))]
    pub fn eval_templ(&self, s: &str) -> Result<String> {
        self.engine.eval_templ(s)
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
    pub fn set_ok(&mut self, ok: bool) {
        self.ok = self.ok && ok;
        self.engine.set_ok(self.ok);
    }

    #[inline]
    pub fn cfg(&self) -> Arc<jobengine::Config> {
        Arc::clone(&self.cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobengine::{JobMeta, ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
    use std::{collections::BTreeMap, env, sync::Arc};
    use uuid::Uuid;

    #[test]
    fn test_eval_from_setup() {
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
            secrets: {
                let mut m = BTreeMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
            env: BTreeMap::new(),
        };
        let ctx = super::ExecutionContext::new(
            env::temp_dir(),
            Arc::new(vec![("key".to_string(), "value".to_string())]),
            job_config.clone(),
        );

        assert_eq!(ctx.eval_expr("1 + 1").unwrap(), CelValue::Int(2));
        assert_eq!(
            ctx.eval_expr("scans.example[0].id").unwrap(),
            CelValue::String(job_config.scans["example"][0].id.to_string())
        );
        assert_eq!(
            ctx.eval_expr("project.id").unwrap(),
            CelValue::String(job_config.project.id.to_string())
        );
        assert_eq!(
            ctx.eval_expr("workflow.id").unwrap(),
            CelValue::String(job_config.workflow.id.to_string())
        );
        assert_eq!(
            ctx.eval_expr("revision.id").unwrap(),
            CelValue::String(job_config.revision.id.to_string())
        );
        assert_eq!(
            ctx.eval_expr("vars.key").unwrap(),
            CelValue::String("value".to_string())
        );
        assert_eq!(
            ctx.eval_expr("name").unwrap(),
            CelValue::String(job_config.name)
        );
        assert_eq!(
            ctx.eval_expr("id").unwrap(),
            CelValue::String(job_config.id.to_string())
        );
    }

    #[test]
    fn test_eval_set_ok() {
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
            secrets: {
                let mut m = BTreeMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
            env: BTreeMap::new(),
        };

        let mut ctx = super::ExecutionContext::new(
            env::temp_dir(),
            Arc::new(vec![("key".to_string(), "value".to_string())]),
            job_config.clone(),
        );

        assert_eq!(ctx.eval_expr("ok").unwrap(), CelValue::Bool(true));

        ctx.set_ok(false);

        assert_eq!(ctx.eval_expr("ok").unwrap(), CelValue::Bool(false));
    }

    #[test]
    fn test_environment_is_set() {
        let project_id = Uuid::now_v7();
        let workflow_id = Uuid::now_v7();
        let revision_id = Uuid::now_v7();
        let job_id = Uuid::now_v7();

        let ctx = super::ExecutionContext::new(
            env::temp_dir(),
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
                secrets: BTreeMap::new(),
                env: {
                    let mut m = BTreeMap::new();
                    m.insert("WORKFLOW_ENV".to_string(), "WORKFLOW_ENV".to_string());
                    m
                },
            },
        );

        let env = ctx.env();
        let got = env.iter().cloned().collect::<BTreeMap<String, String>>();
        assert_eq!(
            got.get("BOUNTYHUB_JOB_ID")
                .expect("BOUNTYHUB_JOB_ID")
                .clone(),
            job_id.to_string()
        );
        assert_eq!(
            got.get("BOUNTYHUB_SCAN_NAME")
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
        assert_eq!(
            got.get("WORKFLOW_ENV").expect("WORKFLOW_ENV").clone(),
            "WORKFLOW_ENV".to_string(),
        )
    }
}
