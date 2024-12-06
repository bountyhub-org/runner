use serde::{Deserialize, Serialize};
use std::error::Error;
use std::{fmt, str::FromStr};

const OPEN_TMPL: &str = "${{";
const CLOSE_TMPL: &str = "}}";

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
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut tmpl = Template { tokens: Vec::new() };

        let mut i = 0;
        loop {
            match input[i..].find(OPEN_TMPL) {
                Some(idx) => {
                    if idx > 0 {
                        tmpl.tokens.push(Token::Lit(input[i..i + idx].to_string()));
                    }
                    i += idx + 3;
                    match input[i..].find(CLOSE_TMPL) {
                        Some(idx) => {
                            let exp = input[i..i + idx].trim();
                            tmpl.tokens.push(Token::Expr(exp.to_string()));
                            i += idx + 2;
                        }
                        None => {
                            return Err(ParseError(format!(
                                "unmatched '{}' at {} in {}",
                                OPEN_TMPL,
                                i - 3,
                                input
                            )));
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

#[derive(Debug, Clone)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "parse error: {}", self.0)
    }
}

impl Error for ParseError {}

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
        assert_eq!(
            err.to_string(),
            "parse error: unmatched '${{' at 6 in hello ${{name!"
        );
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
