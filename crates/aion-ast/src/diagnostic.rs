use serde::{Deserialize, Serialize};

/// Byte-offset span and 1-based line/column coordinates in source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, col: usize) -> Self {
        Self {
            start,
            end,
            line,
            col,
        }
    }

    /// Computes 1-based line and column from a byte offset in `source`.
    pub fn from_range(source: &str, start: usize, end: usize) -> Self {
        let clamped_start = start.min(source.len());
        let clamped_end = end.min(source.len());
        let prefix = &source[..clamped_start];
        let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
        let last_nl = prefix.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let col = source[last_nl..clamped_start].chars().count() + 1;
        Self {
            start: clamped_start,
            end: clamped_end,
            line,
            col,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Lexer,
    Parser,
    Ext,
    Checker,
    Patch,
}

/// Structured compiler diagnostic emitted as JSON for LLM agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub phase: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub message: String,
    pub span: Span,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub found: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_hint: Option<String>,
}

impl Diagnostic {
    pub fn error(
        code: impl Into<String>,
        phase: Phase,
        message: impl Into<String>,
        span: Span,
    ) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Error,
            phase,
            node_id: None,
            message: message.into(),
            span,
            expected: None,
            found: None,
            fix_hint: None,
        }
    }

    pub fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }

    pub fn with_expected(mut self, expected: impl Into<String>) -> Self {
        self.expected = Some(expected.into());
        self
    }

    pub fn with_found(mut self, found: impl Into<String>) -> Self {
        self.found = Some(found.into());
        self
    }

    pub fn with_fix_hint(mut self, fix_hint: impl Into<String>) -> Self {
        self.fix_hint = Some(fix_hint.into());
        self
    }

    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Structured compilation/check report serializable to JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub ok: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub diagnostics: Vec<Diagnostic>,
}

impl DiagnosticReport {
    pub fn from_diagnostics(diagnostics: Vec<Diagnostic>) -> Self {
        let error_count = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let warning_count = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        Self {
            ok: error_count == 0,
            error_count,
            warning_count,
            diagnostics,
        }
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}
