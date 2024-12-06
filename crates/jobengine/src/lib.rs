use cel_interpreter::{
    extractors::This, objects::Value, Context, ExecutionError, FunctionContext, Program,
};
use error_stack::{Context as ErrorContext, Result, ResultExt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
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
    ctx: Context<'static>,
}

impl JobEngine {
    pub fn new(config: &Config) -> Self {
        let mut ctx = Context::default();

        // Variables
        ctx.add_variable("id", config.id.to_string())
            .expect("failed to add id");
        ctx.add_variable("name", config.name.clone())
            .expect("failed to add name");
        ctx.add_variable("project", config.project.clone())
            .expect("failed to add project");
        ctx.add_variable("workflow", config.workflow.clone())
            .expect("failed to add workflow");
        ctx.add_variable("revision", config.revision.clone())
            .expect("failed to add revision");
        ctx.add_variable("scans", config.scans.clone())
            .expect("failed to add scans");
        ctx.add_variable("vars", config.vars.clone())
            .expect("failed to add vars");
        ctx.add_variable("ok", true)
            .expect("failed to add succeeded");
        ctx.add_variable("always", true)
            .expect("failed to add failed");

        // Functions
        ctx.add_function("is_available", is_available);
        ctx.add_function("has_diff", has_diff);

        JobEngine { ctx }
    }

    /// Evaluates the given expression and returns the result
    pub fn eval(&self, expr: &str) -> Result<Value, EvaluationError> {
        let program = Program::compile(expr)
            .change_context(EvaluationError)
            .attach_printable("failed to compile the program")?;

        program
            .execute(&self.ctx)
            .change_context(EvaluationError)
            .attach_printable("failed to execute the program")
    }

    /// Sets the value of the `ok` variable
    pub fn set_ok(&mut self, ok: bool) {
        self.ctx.add_variable("ok", ok).unwrap();
    }

    /// Sets the value of the `name` variable
    pub fn set_job_ctx(&mut self, id: Uuid, name: &str) {
        self.ctx
            .add_variable("id", id.to_string())
            .expect("failed to add id");
        self.ctx
            .add_variable("name", name)
            .expect("failed to add name");
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JobMeta {
    pub id: Uuid,
    pub state: String,
    pub nonce: Option<String>,
}

impl TryFrom<Value> for JobMeta {
    type Error = String;

    fn try_from(value: Value) -> std::result::Result<Self, Self::Error> {
        serde_json::from_value(value.json().map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectMeta {
    pub id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowMeta {
    pub id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowRevisionMeta {
    pub id: Uuid,
}

fn is_available(
    ftx: &FunctionContext,
    This(this): This<Value>,
) -> std::result::Result<bool, ExecutionError> {
    let succeeded_target_scans = serde_json::from_value::<Vec<JobMeta>>(
        this.json().map_err(|err| ftx.error(err.to_string()))?,
    )
    .map_err(|e| ftx.error(e.to_string()))?
    .into_iter()
    .filter(|scan| scan.state == "succeeded")
    .collect::<Vec<_>>();

    let ctx_name = match ftx.ptx.get_variable("name").unwrap() {
        Value::String(name) => name.clone(),
        _ => panic!("expected name to be set"),
    };
    let ctx_id = match ftx.ptx.get_variable("id").unwrap() {
        Value::String(id) => Uuid::parse_str(&id).map_err(|e| ftx.error(e.to_string()))?,
        _ => panic!("expected id to be set"),
    };

    let scans: BTreeMap<String, Vec<JobMeta>> = serde_json::from_value(
        ftx.ptx
            .get_variable("scans")
            .unwrap()
            .json()
            .map_err(|err| ftx.error(err.to_string()))?,
    )
    .map_err(|err| ftx.error(err.to_string()))?;

    let this_jobs = scans
        .get(ctx_name.as_str())
        .ok_or(ftx.error("scan not found"))?
        .iter()
        .find(|job| job.id < ctx_id);

    Ok(!succeeded_target_scans.is_empty()
        && (this_jobs.is_none()
            || this_jobs.unwrap().id < succeeded_target_scans.first().unwrap().id))
}

fn has_diff(
    ftx: &FunctionContext,
    This(this): This<Value>,
) -> std::result::Result<bool, ExecutionError> {
    let succeeded_target_scans: Vec<JobMeta> = serde_json::from_value::<Vec<JobMeta>>(
        this.json().map_err(|err| ftx.error(err.to_string()))?,
    )
    .map_err(|e| ftx.error(e.to_string()))?
    .into_iter()
    .filter(|scan| scan.state == "succeeded")
    .collect::<Vec<_>>();

    Ok(
        (succeeded_target_scans.len() == 1 && succeeded_target_scans[0].nonce.is_some())
            || (succeeded_target_scans.len() > 2
                && succeeded_target_scans[0].nonce != succeeded_target_scans[1].nonce),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn new_config(name: &str) -> Config {
        Config {
            id: Uuid::now_v7(),
            name: name.to_string(),
            vars: BTreeMap::new(),
            scans: BTreeMap::new(),
            project: ProjectMeta { id: Uuid::now_v7() },
            workflow: WorkflowMeta { id: Uuid::now_v7() },
            revision: WorkflowRevisionMeta { id: Uuid::now_v7() },
        }
    }

    #[test]
    fn test_is_available_no_scan() {
        let cfg = new_config("scan1");
        let engine = JobEngine::new(&cfg);
        assert!(engine.eval("scans.scan2.is_available()").is_err());
    }

    #[test]
    fn test_is_available_empty_target() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert("scan2".to_string(), vec![]);
        let mut cfg = new_config("scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_is_available_empty_self() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert("scan1".to_string(), vec![]);

        let mut cfg = new_config("scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_is_available_assigned_self() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let id = Uuid::now_v7();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id,
                state: "assigned".to_string(),
                nonce: None,
            }],
        );

        let mut cfg = new_config("scan1");
        cfg.id = id;
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_is_available_false_on_self() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let mut cfg = new_config("scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan1.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_is_available_false_when_available_before() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let mut cfg = new_config("scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_is_available_true_on_succeeded() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let mut cfg = new_config("scan1");
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
        let mut cfg = new_config("scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_has_diff_no_scan() {
        let cfg = new_config("scan1");
        let engine = JobEngine::new(&cfg);
        assert!(engine.eval("scans.scan2.has_diff()").is_err());
    }

    #[test]
    fn test_has_diff_empty_target() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        scan_jobs.insert("scan2".to_string(), vec![]);
        let mut cfg = new_config("scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan2.is_available()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_has_diff_one_scan_empty_nonce() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let mut cfg = new_config("scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan1.has_diff()").unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_has_diff_one_scan_has_nonce() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: Some("nonce".to_string()),
            }],
        );
        let mut cfg = new_config("scan1");
        cfg.scans = scan_jobs;
        let engine = JobEngine::new(&cfg);
        let result = engine.eval("scans.scan1.has_diff()").unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_ok_and_always() {
        let cfg = new_config("scan1");
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
