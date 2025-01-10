use cellang::{Environment, EnvironmentBuilder, Key, Map, TokenTree, Value};
use error_stack::{Context as ErrorContext, Report, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    /// id of the job from which the evaluation is done
    pub id: Uuid,
    /// The name of the job from which the evaluation is done
    pub name: String,
    /// Variables associated with the workflow
    pub vars: BTreeMap<String, String>,
    /// Scans associated with the workflow
    pub scans: BTreeMap<String, Vec<JobMeta>>,
    /// Project metadata
    pub project: ProjectMeta,
    /// Workflow metadata
    pub workflow: WorkflowMeta,
    /// Workflow revision metadata
    pub revision: WorkflowRevisionMeta,
}

/// JobEngine is a wrapper around the CEL interpreter that provides a context
/// with the necessary variables and functions to evaluate expressions.
pub struct JobEngine {
    ctx: EnvironmentBuilder<'static>,
}

impl JobEngine {
    pub fn new(config: &Config) -> Self {
        let mut ctx = EnvironmentBuilder::default();

        // Variables
        ctx.set_variable("id", config.id.to_string())
            .expect("failed to add id");
        ctx.set_variable("name", config.name.clone())
            .expect("failed to add name");
        ctx.set_variable("project", config.project.clone())
            .expect("failed to add project");
        ctx.set_variable("workflow", config.workflow.clone())
            .expect("failed to add workflow");
        ctx.set_variable("revision", config.revision.clone())
            .expect("failed to add revision");
        ctx.set_variable("scans", config.scans.clone())
            .expect("failed to add scans");
        ctx.set_variable::<&str, Map>(
            "vars",
            config
                .vars
                .clone()
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
        .expect("failed to add vars");
        ctx.set_variable("ok", true)
            .expect("failed to add succeeded");
        ctx.set_variable("always", true)
            .expect("failed to add failed");

        // Functions
        ctx.set_function("is_available", Arc::new(is_available));
        ctx.set_function("has_diff", Arc::new(has_diff));

        JobEngine { ctx }
    }

    /// Evaluates the given expression and returns the result
    pub fn eval(&self, expr: &str) -> Result<Value, EvaluationError> {
        match cellang::eval(&self.ctx.build(), expr) {
            Ok(value) => Ok(value),
            Err(err) => Err(Report::new(EvaluationError).attach_printable(format!("{err:?}"))),
        }
    }

    /// Sets the value of the `ok` variable
    pub fn set_ok(&mut self, ok: bool) {
        self.ctx.set_variable("ok", ok).unwrap();
    }

    /// Sets the value of the `name` variable
    pub fn set_job_ctx(&mut self, id: Uuid, name: &str) {
        self.ctx
            .set_variable("id", id.to_string())
            .expect("failed to add id");
        self.ctx
            .set_variable("name", name)
            .expect("failed to add name");
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JobMeta {
    pub id: Uuid,
    pub state: String,
    pub nonce: Option<String>,
}

impl From<JobMeta> for Value {
    fn from(meta: JobMeta) -> Self {
        Value::Map(
            vec![
                (Key::from("id"), Value::String(meta.id.to_string())),
                (Key::from("state"), Value::String(meta.state)),
                (
                    Key::from("nonce"),
                    meta.nonce.map_or(Value::Null, Value::String),
                ),
            ]
            .into_iter()
            .collect(),
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectMeta {
    pub id: Uuid,
}

impl From<ProjectMeta> for Value {
    fn from(meta: ProjectMeta) -> Self {
        Value::Map(
            vec![(Key::from("id"), Value::String(meta.id.to_string()))]
                .into_iter()
                .collect(),
        )
    }
}

impl From<Value> for ProjectMeta {
    fn from(value: Value) -> Self {
        match value {
            Value::Map(map) => {
                let id = match map.get(&Key::from("id")).unwrap().unwrap() {
                    Value::String(id) => Uuid::parse_str(id).unwrap(),
                    _ => panic!("expected a string"),
                };

                ProjectMeta { id }
            }
            _ => panic!("expected a map"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowMeta {
    pub id: Uuid,
}

impl From<WorkflowMeta> for Value {
    fn from(meta: WorkflowMeta) -> Self {
        Value::Map(
            vec![(Key::from("id"), Value::String(meta.id.to_string()))]
                .into_iter()
                .collect(),
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowRevisionMeta {
    pub id: Uuid,
}

impl From<WorkflowRevisionMeta> for Value {
    fn from(meta: WorkflowRevisionMeta) -> Self {
        Value::Map(
            vec![(Key::from("id"), Value::String(meta.id.to_string()))]
                .into_iter()
                .collect(),
        )
    }
}

fn is_available(
    env: &Environment,
    tokens: &[TokenTree],
) -> std::result::Result<Value, miette::Error> {
    if tokens.len() != 1 {
        miette::bail!("expected 1 argument, got {}", tokens.len());
    }

    let id = match env.lookup_variable("id").unwrap() {
        Value::String(id) => Uuid::parse_str(id).unwrap(),
        _ => miette::bail!("expected a string"),
    };

    let scans = cellang::eval_ast(env, &tokens[0])?;
    let jobs: Vec<JobMeta> = scans.try_from_value()?;
    if jobs.is_empty() {
        return Ok(Value::Bool(false));
    }

    let job = jobs.first().unwrap();
    Ok(Value::Bool(job.state == "succeeded" && id < job.id))
}

fn has_diff(env: &Environment, tokens: &[TokenTree]) -> std::result::Result<Value, miette::Error> {
    if tokens.len() != 1 {
        miette::bail!("expected 1 argument, got {}", tokens.len());
    }

    let scans = cellang::eval_ast(env, &tokens[0])?;
    let scans: Vec<JobMeta> = scans.try_from_value()?;

    let mut scans = scans.into_iter();
    let latest = match scans.next() {
        Some(job) if job.state == "succeeded" => job.clone(),
        _ => return Ok(Value::Bool(false)),
    };

    if latest.nonce.is_none() {
        return Ok(Value::Bool(false));
    }

    // Find the next succeeded job and compare the nonce
    let previous = match scans.find(|job| job.state == "succeeded") {
        Some(job) => job,
        // No previous job so this one is a diff
        None => return Ok(Value::Bool(true)),
    };

    Ok(Value::Bool(previous.nonce != latest.nonce))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn new_config(id: Uuid, name: &str) -> Config {
        Config {
            id,
            name: name.to_string(),
            vars: BTreeMap::new(),
            scans: BTreeMap::new(),
            project: ProjectMeta { id: Uuid::now_v7() },
            workflow: WorkflowMeta { id: Uuid::now_v7() },
            revision: WorkflowRevisionMeta { id: Uuid::now_v7() },
        }
    }

    #[test]
    fn test_is_available_on_notexistent_scan() {
        let cfg = new_config(Uuid::now_v7(), "scan1");
        let engine = JobEngine::new(&cfg);
        assert!(engine.eval("scans.scan2.is_available()").is_err());
    }

    #[test]
    fn test_is_available_on_empty_target() {
        let mut scan_jobs = BTreeMap::new();
        let id = Uuid::now_v7();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id,
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert("scan2".to_string(), vec![]);
        let mut cfg = new_config(Uuid::nil(), "scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_is_available_when_empty_self() {
        let mut scan_jobs = BTreeMap::new();
        let scan2_id = Uuid::now_v7();
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: scan2_id,
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert("scan1".to_string(), vec![]);

        let mut cfg = new_config(Uuid::nil(), "scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_is_available_false_when_available_before() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let mut cfg = new_config(ids[1], "scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_is_available_true_on_succeeded() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let mut cfg = new_config(ids[0], "scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_is_available_when_target_after_but_not_succeeded() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![
                JobMeta {
                    id: ids[2],
                    state: "failed".to_string(),
                    nonce: None,
                },
                JobMeta {
                    id: ids[0],
                    state: "succeeded".to_string(),
                    nonce: None,
                },
            ],
        );
        let mut cfg = new_config(ids[1], "scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_has_diff_no_scan() {
        let cfg = new_config(Uuid::nil(), "scan1");
        let engine = JobEngine::new(&cfg);
        assert!(engine.eval("scans.scan2.has_diff()").is_err());
    }

    #[test]
    fn test_has_diff_empty_target() {
        let mut scan_jobs = BTreeMap::new();
        let id = Uuid::now_v7();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id,
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert("scan2".to_string(), vec![]);
        let mut cfg = new_config(id, "scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_has_diff_one_scan_empty_nonce() {
        let mut scan_jobs = BTreeMap::new();
        let id = Uuid::now_v7();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id,
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let mut cfg = new_config(id, "scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan1.has_diff()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_has_diff_one_scan_has_nonce() {
        let mut scan_jobs = BTreeMap::new();
        let id = Uuid::now_v7();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id,
                state: "succeeded".to_string(),
                nonce: Some("nonce".to_string()),
            }],
        );
        let mut cfg = new_config(id, "scan2");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan1.has_diff()").unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_ok_and_always() {
        let cfg = new_config(Uuid::now_v7(), "scan1");
        let mut engine = JobEngine::new(&cfg);
        let result = engine.eval("ok").unwrap();
        assert_eq!(result, Value::Bool(true));
        let result = engine.eval("always").unwrap();
        assert_eq!(result, Value::Bool(true));
        engine.set_ok(false);
        let result = engine.eval("ok").unwrap();
        assert_eq!(result, Value::Bool(false));
        let result = engine.eval("always").unwrap();
        assert_eq!(result, Value::Bool(true));
    }
}

#[derive(Debug)]
pub struct EvaluationError;

impl fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "evaluation error")
    }
}

impl ErrorContext for EvaluationError {}
