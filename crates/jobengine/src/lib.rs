use cellang::{Environment, EnvironmentBuilder, Key, Map, TokenTree, Value};
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use templ::{Template, Token};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// id of the job from which the evaluation is done
    pub id: Uuid,
    /// The scan name this job belongs to
    pub name: String,
    /// Variables associated with the project
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// Secrets associated with the project
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    /// Workflow environment variables
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Custom inputs
    pub inputs: Option<BTreeMap<String, InputValue>>,
    /// Scans associated with the workflow
    pub scans: BTreeMap<String, Vec<JobMeta>>,
    /// Project metadata
    pub project: ProjectMeta,
    /// Workflow metadata
    pub workflow: WorkflowMeta,
    /// Revision metadata
    pub revision: WorkflowRevisionMeta,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type", content = "value")]
pub enum InputValue {
    String(String),
    Bool(bool),
}

/// JobEngine is a wrapper around the CEL interpreter that provides a context
/// with the necessary variables and functions to evaluate expressions.
#[derive(Debug)]
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
        ctx.set_variable(
            "vars",
            config
                .vars
                .clone()
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect::<Map>(),
        )
        .expect("failed to add vars");
        ctx.set_variable(
            "secrets",
            config
                .secrets
                .clone()
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect::<Map>(),
        )
        .expect("failed to add secrets");
        ctx.set_variable(
            "inputs",
            match &config.inputs {
                Some(inputs) => inputs
                    .clone()
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k.into(),
                            match v {
                                InputValue::String(s) => s.into(),
                                InputValue::Bool(b) => b.into(),
                            },
                        )
                    })
                    .collect::<Map>(),
                None => Map::new(),
            },
        )
        .expect("failed to add inputs");
        ctx.set_variable("ok", true)
            .expect("failed to add succeeded");
        ctx.set_variable("always", true)
            .expect("failed to add failed");

        // Functions
        ctx.set_function("is_available", Arc::new(is_available));
        ctx.set_function("has_diff", Arc::new(has_diff));

        JobEngine { ctx }
    }

    pub fn eval_templ(&self, s: &str) -> Result<String> {
        let Template { tokens } = s.parse().wrap_err("failed to parse expression")?;

        let mut output = String::new();

        for token in tokens {
            match token {
                Token::Lit(lit) => output.push_str(&lit),
                Token::Expr(expr) => {
                    let value = self.eval_expr(&expr)?;

                    let value = match value {
                        Value::Int(val) => val.to_string(),
                        Value::Uint(val) => val.to_string(),
                        Value::Double(val) => val.to_string(),
                        Value::String(val) => val.to_string(),
                        Value::Bool(val) => val.to_string(),
                        Value::Duration(val) => val.to_string(),
                        Value::Timestamp(val) => val.to_string(),
                        Value::Null => "".to_string(),
                        val => return Err(miette::miette!("Unsupported value type: {val:?}")),
                    };

                    output.push_str(&value);
                }
            }
        }

        Ok(output)
    }

    /// Evaluates the given expression and returns the result
    pub fn eval_expr(&self, expr: &str) -> Result<Value> {
        cellang::eval(&self.ctx.build(), expr).wrap_err("failed to evaluate expression")
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
    pub artifacts: BTreeMap<String, ArtifactMeta>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArtifactMeta {
    pub checksum: Option<String>,
}

impl From<JobMeta> for Value {
    fn from(meta: JobMeta) -> Self {
        let artifacts = Value::Map(
            meta.artifacts
                .into_iter()
                .map(|(k, v)| {
                    (
                        Key::from(k),
                        Value::Map(
                            vec![(
                                Key::from("checksum"),
                                Value::String(v.checksum.unwrap_or_default()),
                            )]
                            .into_iter()
                            .collect(),
                        ),
                    )
                })
                .collect(),
        );

        Value::Map(
            vec![
                (Key::from("id"), Value::String(meta.id.to_string())),
                (Key::from("state"), Value::String(meta.state)),
                (Key::from("artifacts"), artifacts),
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

/// is_available checks if the target job is available based on the current job's state and the
/// state of the target job.
///
/// If in the context of the current scan, the target scan has a job that succeeeded after the last
/// finished instance of the current scan, it returns true.
/// Otherwise, it returns false.
fn is_available(
    env: &Environment,
    tokens: &[TokenTree],
) -> std::result::Result<Value, miette::Error> {
    if tokens.len() != 1 {
        miette::bail!("expected 1 argument, got {}", tokens.len());
    }

    let scans = cellang::eval_ast(env, &tokens[0])?;

    let target_job: Option<Uuid> = scans
        .try_from_value::<Vec<JobMeta>>()?
        .into_iter()
        .find_map(|job| {
            if job.state == "succeeded" {
                Some(job.id)
            } else {
                None
            }
        });

    if target_job.is_none() {
        return Ok(false.into());
    }

    let this_job = match this_jobs(env)?.first().cloned() {
        Some(job) => job,
        None => return Ok(true.into()),
    };

    Ok((this_job.id < target_job.unwrap()).into())
}

fn has_diff(env: &Environment, tokens: &[TokenTree]) -> std::result::Result<Value, miette::Error> {
    if tokens.len() != 2 {
        miette::bail!("expected 2 arguments, got {}", tokens.len());
    }

    let this_id = match env.lookup_variable("id").expect("id field is missing") {
        Value::String(name) => Uuid::parse_str(name)
            .into_diagnostic()
            .wrap_err("failed to parse this id")?,
        _ => miette::bail!("name field is missing"),
    };

    let artifact_name = match cellang::eval_ast(env, &tokens[1])?.try_value()? {
        Value::String(name) => name.to_owned(),
        _ => miette::bail!("expected a string as artifact name"),
    };

    // evaluate the target scan jobs
    let scans = cellang::eval_ast(env, &tokens[0])?;
    let target_jobs: Vec<JobMeta> = scans
        .try_from_value::<Vec<JobMeta>>()?
        .into_iter()
        .filter(|job| job.state == "succeeded")
        .filter(|job| {
            job.artifacts
                .iter()
                .any(|(name, _)| **name == artifact_name)
        })
        .collect::<_>();

    // if there is no succeeded job, return false
    if target_jobs.is_empty() {
        return Ok(false.into());
    }

    // if this target doesn't contain artifacts anyway, return false
    if target_jobs.first().unwrap().artifacts.is_empty() {
        return Ok(false.into());
    }

    // if it does, get the current scan jobs without this job
    let this_jobs = this_jobs(env)?
        .into_iter()
        .filter(|job| job.id != this_id)
        .collect::<Vec<_>>();

    if this_jobs.is_empty() {
        return Ok(true.into());
    }

    // do not resolve to true if this job already executed after the target job
    if this_jobs.first().unwrap().id > target_jobs.first().unwrap().id {
        return Ok(false.into());
    }

    // compare the latest two succeeded jobs' artifact checksums
    let mut target_last_two = target_jobs.iter().take(2);
    let target_latest = target_last_two
        .next()
        .unwrap()
        .artifacts
        .iter()
        .find_map(|(name, meta)| {
            if name == &artifact_name {
                meta.checksum.clone()
            } else {
                None
            }
        })
        .unwrap_or_default();

    let target_previous = target_last_two
        .next()
        .and_then(|job| {
            job.artifacts.iter().find_map(|(name, meta)| {
                if name == &artifact_name {
                    meta.checksum.clone()
                } else {
                    None
                }
            })
        })
        .unwrap_or_default();

    Ok((target_latest != target_previous).into())
}

/// this_job retrieves the metadata of the current job from the environment.
/// It is currently not meant to be exposed, therefore, it is not present in the list of functions.
fn this_jobs(env: &Environment) -> std::result::Result<Vec<JobMeta>, miette::Error> {
    let name = match env.lookup_variable("name").expect("name field is missing") {
        Value::String(name) => name,
        _ => miette::bail!("name field is missing"),
    };

    match env.lookup_variable("scans").unwrap() {
        Value::Map(m) => {
            let value = match m
                .get(&Key::from(name))
                .expect("key should be of correct type")
            {
                Some(value) => value,
                None => miette::bail!("map value for key {name} is missing"),
            };

            Ok(cellang::try_from_value(value.clone())?)
        }
        _ => miette::bail!("scans field is missing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_config(id: Uuid, name: &str, scans: BTreeMap<String, Vec<JobMeta>>) -> Config {
        Config {
            id,
            name: name.to_string(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            env: BTreeMap::new(),
            inputs: None,
            scans,
            project: ProjectMeta { id: Uuid::now_v7() },
            workflow: WorkflowMeta { id: Uuid::now_v7() },
            revision: WorkflowRevisionMeta { id: Uuid::now_v7() },
        }
    }

    fn assert_eval(cfg: &Config, expr: &str, expected: bool) {
        let engine = JobEngine::new(cfg);
        let result = engine.eval_expr(expr).expect("failed to evaluate {expr}");
        assert_eq!(result, Value::Bool(expected));
    }

    #[test]
    fn test_is_available_target_empty() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert("scan2".to_string(), vec![]);

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", false);
    }

    #[test]
    fn test_is_available_self_empty() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert("scan1".to_string(), vec![]);

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", true);
    }

    #[test]
    fn test_is_available_both_empty() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert("scan1".to_string(), vec![]);
        scan_jobs.insert("scan2".to_string(), vec![]);
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", false);
    }

    #[test]
    fn test_is_available_target_before_self() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", false);
    }

    #[test]
    fn test_is_available_self_before_target_but_target_failed() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "failed".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", false);
    }

    #[test]
    fn test_is_available_self_failed_before_target() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "failed".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", true);
    }

    #[test]
    fn test_is_available_self_failed_after_target() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "failed".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", false);
    }

    #[test]
    fn test_is_available_target_after_self() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", true);
    }

    #[test]
    fn test_has_diff_target_empty() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert("scan2".to_string(), vec![]);

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.has_diff('artifact1')", false);
    }

    #[test]
    fn test_has_diff_self_empty() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                artifacts: {
                    let mut m = BTreeMap::default();
                    m.insert(
                        "artifact1".to_string(),
                        ArtifactMeta {
                            checksum: Some("checksum1".to_string()),
                        },
                    );
                    m
                },
            }],
        );
        scan_jobs.insert("scan1".to_string(), vec![]);

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.has_diff('artifact1')", true);
    }

    #[test]
    fn test_has_diff_both_empty() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert("scan1".to_string(), vec![]);
        scan_jobs.insert("scan2".to_string(), vec![]);
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.has_diff('artifact1')", false);
    }

    #[test]
    fn test_has_diff_no_artifact_in_target() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.has_diff('artifact1')", false);
    }

    #[test]
    fn test_has_diff_but_last_instance_already_executed() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.has_diff('artifact1')", false);
    }

    #[test]
    fn test_has_diff_single_artifact() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                artifacts: {
                    let mut m = BTreeMap::default();
                    m.insert(
                        "artifact1".to_string(),
                        ArtifactMeta {
                            checksum: Some("checksum1".to_string()),
                        },
                    );
                    m
                },
            }],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.has_diff('artifact1')", true);
    }

    #[test]
    fn test_has_diff_multiple_artifacts() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![
                JobMeta {
                    id: ids[2],
                    state: "succeeded".to_string(),
                    artifacts: {
                        let mut m = BTreeMap::default();
                        m.insert(
                            "artifact1".to_string(),
                            ArtifactMeta {
                                checksum: Some("checksum1".to_string()),
                            },
                        );
                        m
                    },
                },
                JobMeta {
                    id: ids[0],
                    state: "succeeded".to_string(),
                    artifacts: {
                        let mut m = BTreeMap::default();
                        m.insert(
                            "artifact1".to_string(),
                            ArtifactMeta {
                                checksum: Some("checksum2".to_string()),
                            },
                        );
                        m
                    },
                },
            ],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.has_diff('artifact1')", true);
    }

    #[test]
    fn test_has_no_diff_multiple_artifacts() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "succeeded".to_string(),
                artifacts: BTreeMap::default(),
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![
                JobMeta {
                    id: ids[2],
                    state: "succeeded".to_string(),
                    artifacts: {
                        let mut m = BTreeMap::default();
                        m.insert(
                            "artifact1".to_string(),
                            ArtifactMeta {
                                checksum: Some("checksum1".to_string()),
                            },
                        );
                        m
                    },
                },
                JobMeta {
                    id: ids[0],
                    state: "succeeded".to_string(),
                    artifacts: {
                        let mut m = BTreeMap::default();
                        m.insert(
                            "artifact1".to_string(),
                            ArtifactMeta {
                                checksum: Some("checksum1".to_string()),
                            },
                        );
                        m
                    },
                },
            ],
        );

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.has_diff('artifact1')", false);
    }

    #[test]
    fn test_ok_and_always() {
        let scan_jobs = BTreeMap::new();
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        let mut engine = JobEngine::new(&cfg);
        let result = engine.eval_expr("ok").unwrap();
        assert_eq!(result, Value::Bool(true));
        let result = engine.eval_expr("always").unwrap();
        assert_eq!(result, Value::Bool(true));
        engine.set_ok(false);
        let result = engine.eval_expr("ok").unwrap();
        assert_eq!(result, Value::Bool(false));
        let result = engine.eval_expr("always").unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_eval_empty_inputs() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert("scan1".to_string(), vec![]);
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        let engine = JobEngine::new(&cfg);
        let result = engine
            .eval_expr("has(inputs.test)")
            .expect("has should not fail");
        assert_eq!(
            result,
            Value::Bool(false),
            "expected has(inputs.test) to be false"
        );

        let result = engine.eval_expr("inputs.test");
        assert!(result.is_err(), "expected error, got {result:?}");
    }

    #[test]
    fn test_eval_set_inputs() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert("scan1".to_string(), vec![]);
        let mut cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        cfg.inputs = Some({
            let mut m = BTreeMap::new();
            m.insert(
                "key_str".to_string(),
                InputValue::String("example".to_string()),
            );
            m.insert("key_bool".to_string(), InputValue::Bool(true));
            m
        });
        let engine = JobEngine::new(&cfg);

        let result = engine
            .eval_expr("has(inputs.key_str)")
            .expect("has should not fail");
        assert_eq!(
            result,
            Value::Bool(true),
            "expected inputs to contain key_str"
        );

        let result = engine
            .eval_expr("inputs.key_str")
            .expect("expected inputs.key_str to exist");
        assert_eq!(result, Value::String("example".to_string()));

        let result = engine
            .eval_expr("has(inputs.key_bool)")
            .expect("has should not fail");
        assert_eq!(
            result,
            Value::Bool(true),
            "expected inputs to contain key_bool"
        );

        let result = engine
            .eval_expr("inputs.key_bool")
            .expect("expected inputs.key_bool to exist");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_eval_templ_var() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert("scan1".to_string(), vec![]);
        let mut cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        cfg.vars
            .insert("EXAMPLE_KEY".to_string(), "EXAMPLE_VALUE".to_string());
        let engine = JobEngine::new(&cfg);

        let expr = "test str";
        let got = engine
            .eval_templ(expr)
            .unwrap_or_else(|_| panic!("failed to evaluate template: {expr}"));
        assert_eq!(expr, got);

        let expr = "input has '${{ vars.EXAMPLE_KEY }}' value";
        let got = engine
            .eval_templ(expr)
            .unwrap_or_else(|_| panic!("failed to evaluate template: {expr}"));
        let want = "input has 'EXAMPLE_VALUE' value";
        assert_eq!(want, got);
    }

    #[test]
    fn test_eval_templ_secrret() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert("scan1".to_string(), vec![]);
        let mut cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        cfg.secrets
            .insert("EXAMPLE_KEY".to_string(), "EXAMPLE_VALUE".to_string());
        let engine = JobEngine::new(&cfg);

        let expr = "test str";
        let got = engine
            .eval_templ(expr)
            .unwrap_or_else(|_| panic!("failed to evaluate template: {expr}"));
        assert_eq!(expr, got);

        let expr = "input has '${{ secrets.EXAMPLE_KEY }}' value";
        let got = engine
            .eval_templ(expr)
            .unwrap_or_else(|_| panic!("failed to evaluate template: {expr}"));
        let want = "input has 'EXAMPLE_VALUE' value";
        assert_eq!(want, got);
    }
}
