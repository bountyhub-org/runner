use cellang::{Environment, EnvironmentBuilder, Key, Map, TokenTree, Value};
use miette::{Result, WrapErr};
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
    /// The name of the job from which the evaluation is done
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
    /// Workflow revision metadata
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

    let name = match env.lookup_variable("name").expect("name field is missing") {
        Value::String(name) => name,
        _ => miette::bail!("name field is missing"),
    };

    let this_scan_latest_id = match env.lookup_variable("scans").unwrap() {
        Value::Map(m) => {
            let value = match m
                .get(&Key::from(name))
                .expect("key should be of correct type")
            {
                Some(value) => value,
                None => miette::bail!("map value for key {name} is missing"),
            };

            let jobs: Vec<JobMeta> = cellang::try_from_value(value.clone())?;
            jobs.first().map(|job| Some(job.id)).unwrap_or(None)
        }
        _ => miette::bail!("scans field is missing"),
    };

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

    if this_scan_latest_id.is_none() {
        return Ok(true.into());
    }

    Ok((this_scan_latest_id.unwrap() < target_job.unwrap()).into())
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
                nonce: None,
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
                nonce: None,
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
                nonce: None,
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                nonce: None,
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
                nonce: None,
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[1],
                state: "failed".to_string(),
                nonce: None,
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
                nonce: None,
            }],
        );
        scan_jobs.insert(
            "scan2".to_string(),
            vec![JobMeta {
                id: ids[0],
                state: "succeeded".to_string(),
                nonce: None,
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

        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan2.is_available()", true);
    }

    #[test]
    fn test_has_diff_on_empty() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert("scan1".to_string(), vec![]);
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan1.has_diff()", false);
    }

    #[test]
    fn test_has_diff_on_one_failed() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "failed".to_string(),
                nonce: None,
            }],
        );
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan1.has_diff()", false);
    }

    #[test]
    fn test_has_diff_on_one_no_nonce() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: None,
            }],
        );
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan1.has_diff()", false);
    }

    #[test]
    fn test_has_diff_on_one_with_nonce() {
        let mut scan_jobs = BTreeMap::new();
        scan_jobs.insert(
            "scan1".to_string(),
            vec![JobMeta {
                id: Uuid::now_v7(),
                state: "succeeded".to_string(),
                nonce: Some("nonce".to_string()),
            }],
        );
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan1.has_diff()", true);
    }

    #[test]
    fn test_has_diff_on_two_with_same_nonce() {
        let mut scan_jobs = BTreeMap::new();
        let nonce = "nonce".to_string();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![
                JobMeta {
                    id: ids[1],
                    state: "succeeded".to_string(),
                    nonce: Some(nonce.clone()),
                },
                JobMeta {
                    id: ids[0],
                    state: "succeeded".to_string(),
                    nonce: Some(nonce),
                },
            ],
        );
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan1.has_diff()", false);
    }

    #[test]
    fn test_has_diff_on_two_with_latest_failed() {
        let mut scan_jobs = BTreeMap::new();
        let nonce = "nonce".to_string();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![
                JobMeta {
                    id: ids[1],
                    state: "succeeded".to_string(),
                    nonce: Some(nonce.clone()),
                },
                JobMeta {
                    id: ids[0],
                    state: "failed".to_string(),
                    nonce: Some(nonce),
                },
            ],
        );
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan1.has_diff()", true);
    }

    #[test]
    fn test_has_diff_on_latest_failed_scan() {
        let mut scan_jobs = BTreeMap::new();
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        scan_jobs.insert(
            "scan1".to_string(),
            vec![
                JobMeta {
                    id: ids[1],
                    state: "failed".to_string(),
                    nonce: Some("latest".to_string()),
                },
                JobMeta {
                    id: ids[0],
                    state: "succeeded".to_string(),
                    nonce: Some("previous".to_string()),
                },
            ],
        );
        let cfg = test_config(Uuid::now_v7(), "scan1", scan_jobs);
        assert_eval(&cfg, "scans.scan1.has_diff()", false);
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
