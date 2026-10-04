//! Trends formulas: arithmetic over series letters (`A` = series 0, …).
//! Parsed into a small tree in Rust — never evaluated by a database.
//! Division by zero yields 0, matching PostHog's charts.

use super::QueryError;

const MAX_DEPTH: usize = 32;
const MAX_TOKENS: usize = 500;

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Number(f64),
    Series(usize),
    Negate(Box<Node>),
    Binary(Box<Node>, char, Box<Node>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Formula {
    root: Node,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Letter(usize),
    Op(char),
    Open,
    Close,
}

fn tokenize(text: &str, series: usize) -> Result<Vec<Token>, QueryError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if tokens.len() > MAX_TOKENS {
            return Err(QueryError::invalid("formula is too long"));
        }
        let c = chars[index];
        match c {
            ' ' | '\t' | '\n' | '\r' => index += 1,
            '+' | '-' | '*' | '/' => {
                tokens.push(Token::Op(c));
                index += 1;
            }
            '(' => {
                tokens.push(Token::Open);
                index += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                index += 1;
            }
            '0'..='9' | '.' => {
                let start = index;
                while index < chars.len() && (chars[index].is_ascii_digit() || chars[index] == '.')
                {
                    index += 1;
                }
                let literal: String = chars[start..index].iter().collect();
                let number: f64 = literal.parse().map_err(|_| {
                    QueryError::invalid(format!("invalid number `{literal}` in formula"))
                })?;
                tokens.push(Token::Number(number));
            }
            'A'..='Z' | 'a'..='z' => {
                let letter = c.to_ascii_uppercase() as usize - 'A' as usize;
                if letter >= series {
                    return Err(QueryError::invalid(format!(
                        "formula refers to series {c}, but the query has {series} series"
                    )));
                }
                tokens.push(Token::Letter(letter));
                index += 1;
            }
            other => {
                return Err(QueryError::invalid(format!(
                    "unexpected `{other}` in formula"
                )));
            }
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn expression(&mut self, depth: usize) -> Result<Node, QueryError> {
        if depth > MAX_DEPTH {
            return Err(QueryError::invalid("formula is nested too deeply"));
        }
        let mut left = self.term(depth + 1)?;
        while let Some(Token::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.position += 1;
            let right = self.term(depth + 1)?;
            left = Node::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn term(&mut self, depth: usize) -> Result<Node, QueryError> {
        let mut left = self.unary(depth + 1)?;
        while let Some(Token::Op(op @ ('*' | '/'))) = self.peek().cloned() {
            self.position += 1;
            let right = self.unary(depth + 1)?;
            left = Node::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self, depth: usize) -> Result<Node, QueryError> {
        if depth > MAX_DEPTH {
            return Err(QueryError::invalid("formula is nested too deeply"));
        }
        match self.peek().cloned() {
            Some(Token::Op('-')) => {
                self.position += 1;
                Ok(Node::Negate(Box::new(self.unary(depth + 1)?)))
            }
            Some(Token::Op('+')) => {
                self.position += 1;
                self.unary(depth + 1)
            }
            Some(Token::Number(number)) => {
                self.position += 1;
                Ok(Node::Number(number))
            }
            Some(Token::Letter(index)) => {
                self.position += 1;
                Ok(Node::Series(index))
            }
            Some(Token::Open) => {
                self.position += 1;
                let inner = self.expression(depth + 1)?;
                if self.peek() != Some(&Token::Close) {
                    return Err(QueryError::invalid("unbalanced parentheses in formula"));
                }
                self.position += 1;
                Ok(inner)
            }
            _ => Err(QueryError::invalid("incomplete formula")),
        }
    }
}

impl Formula {
    pub fn parse(text: &str, series: usize) -> Result<Self, QueryError> {
        let tokens = tokenize(text, series)?;
        if tokens.is_empty() {
            return Err(QueryError::invalid("formula is empty"));
        }
        let mut parser = Parser {
            tokens,
            position: 0,
        };
        let root = parser.expression(0)?;
        if parser.position != parser.tokens.len() {
            return Err(QueryError::invalid("unexpected trailing input in formula"));
        }
        Ok(Self { root })
    }

    /// Evaluate with `values[i]` as series `i`.
    pub fn evaluate(&self, values: &[f64]) -> f64 {
        fn eval(node: &Node, values: &[f64]) -> f64 {
            match node {
                Node::Number(number) => *number,
                Node::Series(index) => values.get(*index).copied().unwrap_or(0.0),
                Node::Negate(inner) => -eval(inner, values),
                Node::Binary(left, op, right) => {
                    let (left, right) = (eval(left, values), eval(right, values));
                    match op {
                        '+' => left + right,
                        '-' => left - right,
                        '*' => left * right,
                        _ => {
                            if right == 0.0 {
                                0.0
                            } else {
                                left / right
                            }
                        }
                    }
                }
            }
        }
        let value = eval(&self.root, values);
        if value.is_finite() { value } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_parentheses_and_unary() {
        let values = [10.0, 4.0, 2.0];
        let cases = [
            ("A + B * C", 18.0),
            ("(A + B) * C", 28.0),
            ("A / B * 100", 250.0),
            ("-A + B", -6.0),
            ("a - -b", 14.0),
            ("A / (B - 4)", 0.0),
            ("2.5 * C", 5.0),
        ];
        for (text, expected) in cases {
            let formula = Formula::parse(text, 3).unwrap();
            assert_eq!(formula.evaluate(&values), expected, "{text}");
        }
    }

    #[test]
    fn rejects_unsafe_or_malformed_input() {
        for text in [
            "",
            "A +",
            "(A",
            "A)",
            "D",
            "A; DROP TABLE x",
            "sqrt(A)",
            "A ** B",
            "1..2",
            &"(".repeat(100),
        ] {
            assert!(Formula::parse(text, 3).is_err(), "{text}");
        }
    }
}
