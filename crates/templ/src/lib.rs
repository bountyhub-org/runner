use miette::{Error, LabeledSpan};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

const OPEN_TEMPL: &str = "${{";
const CLOSE_TEMPL: &str = "}}";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Template {
    pub tokens: Vec<Token>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "value")]
pub enum Token {
    Lit(String),
    Expr(String),
}

impl FromStr for Template {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut tmpl = Template { tokens: Vec::new() };

        let mut i = 0;
        loop {
            match input[i..].find(OPEN_TEMPL) {
                Some(idx) => {
                    if idx > 0 {
                        tmpl.tokens.push(Token::Lit(input[i..i + idx].to_string()));
                    }
                    i += idx + 3;
                    match input[i..].find(CLOSE_TEMPL) {
                        Some(idx) => {
                            let exp = input[i..i + idx].trim();
                            tmpl.tokens.push(Token::Expr(exp.to_string()));
                            i += idx + 2;
                        }
                        None => {
                            return Err(miette::miette! {
                                labels = vec![LabeledSpan::at(
                                    idx..i,
                                    "Open delimiter"
                                )],
                                help = format!("Template must be closed with {CLOSE_TEMPL} delimiter"),
                                "Missing closing template delimiter"
                            }.with_source_code(input.to_string()));
                        }
                    }
                }
                None => {
                    if i < input.len() {
                        tmpl.tokens.push(Token::Lit(input[i..].to_string()));
                    }
                    break;
                }
            }
        }

        Ok(tmpl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        let tmpl = Template::from_str("hello ${{name}}!").unwrap();
        assert_eq!(
            tmpl,
            Template {
                tokens: vec![
                    Token::Lit("hello ".to_string()),
                    Token::Expr("name".to_string()),
                    Token::Lit("!".to_string())
                ]
            }
        );
    }

    #[test]
    fn test_missing_closing() {
        let err = Template::from_str("hello ${{name!").unwrap_err();
        assert!(err
            .to_string()
            .contains("Missing closing template delimiter"));
    }

    #[test]
    fn test_missing_opening() {
        let tmpl = Template::from_str("hello name}}!").unwrap();
        assert_eq!(
            tmpl,
            Template {
                tokens: vec![Token::Lit("hello name}}!".to_string())],
            },
        );
    }
}
