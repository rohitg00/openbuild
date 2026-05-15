use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Default,
    AcceptEdits,
    Auto,
    DontAsk,
    BypassPermissions,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub tool: String,
    pub arg_pattern: Option<String>,
}

impl Rule {
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if let Some((tool, rest)) = s.split_once('(') {
            let arg = rest
                .strip_suffix(')')
                .ok_or_else(|| anyhow!("rule missing closing paren: {s}"))?;
            Ok(Self {
                tool: tool.trim().to_string(),
                arg_pattern: Some(arg.trim().to_string()),
            })
        } else {
            Ok(Self {
                tool: s.to_string(),
                arg_pattern: None,
            })
        }
    }

    pub fn matches(&self, tool: &str, args: &serde_json::Value) -> bool {
        if !self.tool.eq_ignore_ascii_case(tool) && self.tool != "*" {
            return false;
        }
        let pat = match &self.arg_pattern {
            Some(p) => p,
            None => return true,
        };
        if pat == "*" {
            return true;
        }
        let s = match args {
            serde_json::Value::String(s) => s.clone(),
            _ => serde_json::to_string(args).unwrap_or_default(),
        };
        glob_match(pat, &s)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Engine {
    pub mode: Mode,
    pub allow: Vec<Rule>,
    pub deny: Vec<Rule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
    Plan,
}

impl Engine {
    pub fn add_allow(&mut self, raw: &str) -> Result<()> {
        self.allow.push(Rule::parse(raw)?);
        Ok(())
    }

    pub fn add_deny(&mut self, raw: &str) -> Result<()> {
        self.deny.push(Rule::parse(raw)?);
        Ok(())
    }

    pub fn evaluate(&self, tool: &str, args: &serde_json::Value, is_write: bool) -> Decision {
        for r in &self.deny {
            if r.matches(tool, args) {
                return Decision::Deny;
            }
        }
        if matches!(self.mode, Mode::Plan) {
            if is_write || is_side_effect_tool(tool) {
                return Decision::Plan;
            }
            return Decision::Allow;
        }
        if self.allow.iter().any(|r| r.matches(tool, args)) {
            return Decision::Allow;
        }
        match self.mode {
            Mode::Plan => Decision::Plan,
            Mode::BypassPermissions | Mode::DontAsk => Decision::Allow,
            Mode::Auto => Decision::Allow,
            Mode::AcceptEdits => {
                if is_write {
                    Decision::Allow
                } else {
                    Decision::Ask
                }
            }
            Mode::Default => Decision::Ask,
        }
    }
}

fn is_side_effect_tool(tool: &str) -> bool {
    matches!(
        tool,
        "run_terminal_cmd" | "bash" | "write_file" | "edit_file" | "multi_edit"
    )
}

fn glob_match(pattern: &str, s: &str) -> bool {
    let mut pi = pattern.chars().peekable();
    let mut si = s.chars().peekable();
    loop {
        match pi.peek().copied() {
            None => return si.peek().is_none(),
            Some('*') => {
                pi.next();
                if pi.peek().is_none() {
                    return true;
                }
                while si.peek().is_some() {
                    if glob_match(
                        &pi.clone().collect::<String>(),
                        &si.clone().collect::<String>(),
                    ) {
                        return true;
                    }
                    si.next();
                }
                return false;
            }
            Some('?') => {
                if si.next().is_none() {
                    return false;
                }
                pi.next();
            }
            Some(c) => {
                if si.next() != Some(c) {
                    return false;
                }
                pi.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_rule_with_pattern() {
        let r = Rule::parse("Bash(git status)").unwrap();
        assert_eq!(r.tool, "Bash");
        assert_eq!(r.arg_pattern.as_deref(), Some("git status"));
    }

    #[test]
    fn parse_rule_bare() {
        let r = Rule::parse("Read").unwrap();
        assert_eq!(r.tool, "Read");
        assert_eq!(r.arg_pattern, None);
    }

    #[test]
    fn rule_matches_glob() {
        let r = Rule::parse("Bash(git *)").unwrap();
        assert!(r.matches("Bash", &json!("git status")));
        assert!(r.matches("Bash", &json!("git push origin main")));
        assert!(!r.matches("Bash", &json!("rm -rf /")));
    }

    #[test]
    fn deny_overrides_allow() {
        let mut e = Engine::default();
        e.add_allow("Bash(*)").unwrap();
        e.add_deny("Bash(rm *)").unwrap();
        assert_eq!(
            e.evaluate("Bash", &json!("rm -rf foo"), false),
            Decision::Deny
        );
        assert_eq!(e.evaluate("Bash", &json!("ls"), false), Decision::Allow);
    }

    #[test]
    fn mode_drives_unmatched() {
        let mut e = Engine {
            mode: Mode::BypassPermissions,
            ..Default::default()
        };
        assert_eq!(e.evaluate("X", &json!({}), false), Decision::Allow);
        e.mode = Mode::Default;
        assert_eq!(e.evaluate("X", &json!({}), false), Decision::Ask);
    }

    #[test]
    fn plan_mode_blocks_writes() {
        let e = Engine {
            mode: Mode::Plan,
            ..Default::default()
        };
        assert_eq!(e.evaluate("read_file", &json!({}), false), Decision::Allow);
        assert_eq!(e.evaluate("write_file", &json!({}), true), Decision::Plan);
        assert_eq!(e.evaluate("bash", &json!("ls"), false), Decision::Plan);
        assert_eq!(e.evaluate("grep", &json!({}), false), Decision::Allow);
    }
}
