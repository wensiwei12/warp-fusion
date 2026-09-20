/// An actual alert to compare against oracle expectations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActualAlert {
    pub rule_name: String,
    pub score: f64,
    pub entity_type: String,
    pub entity_id: String,
    pub origin: String,
    pub fired_at: String,
}

/// Summary statistics of the verify comparison.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VerifySummary {
    pub expected_total: usize,
    /// 期望侧被剔除的**中间管道输出**条数（`OracleAlert::intermediate`）：它们不落 sink、
    /// 不参与比较（见 [`crate::verify::verify`]）。单独计数，避免「期望 0 条」被读成
    /// 「场景没有期望」——真实的 0 条与「全是中间输出被剔掉」是两回事。
    #[serde(default)]
    pub expected_skipped_intermediate: usize,
    pub actual_total: usize,
    pub matched: usize,
    pub missing: usize,
    pub unexpected: usize,
    pub field_mismatch: usize,
}

/// Detail record for a missing or unexpected alert.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AlertDetail {
    pub rule_name: String,
    pub entity_type: String,
    pub entity_id: String,
    pub score: f64,
    pub time: String,
}

/// Detail record for a score mismatch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MismatchDetail {
    pub rule_name: String,
    pub entity_type: String,
    pub entity_id: String,
    pub expected_score: f64,
    pub actual_score: f64,
    pub expected_time: String,
    pub actual_time: String,
}

/// Full verification report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VerifyReport {
    /// 回执 schema 版本（L1：与 `wfl test` 的 wfl-test-report/v1 同风格版本化）。
    /// 后续新增字段都是**加性**的（`#[serde(default)]`），故不随字段增加而升版。
    pub schema: String,
    pub status: String,
    /// 两侧都没有可比对的告警（无证据）。`status = pass` 只在显式 `--allow-empty` 下
    /// 与它同时成立——见 [`crate::verify::EmptyPolicy`]。
    #[serde(default)]
    pub empty: bool,
    /// 裁定说明：这次对拍**有没有证据**、无证据时怎么放行。写进报告而不是
    /// stderr——调用方常把 stdout/stderr 合并重定向成 JSON 文件，往 stderr 写会
    /// 直接毁掉那份 JSON。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub summary: VerifySummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_details: Vec<AlertDetail>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unexpected_details: Vec<AlertDetail>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mismatch_details: Vec<MismatchDetail>,
}

impl VerifyReport {
    /// Render the report as a PR-friendly Markdown table.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("## wfgen Verify Report\n\n");
        md.push_str(&format!("**Status**: {}\n\n", self.status.to_uppercase()));
        if let Some(note) = &self.note {
            md.push_str(&format!("**Note**: {note}\n\n"));
        }

        // Summary table
        md.push_str("### Summary\n\n");
        md.push_str("| Metric | Count |\n");
        md.push_str("|--------|-------|\n");
        md.push_str(&format!(
            "| Expected total | {} |\n",
            self.summary.expected_total
        ));
        if self.summary.expected_skipped_intermediate > 0 {
            md.push_str(&format!(
                "| Expected skipped (intermediate) | {} |\n",
                self.summary.expected_skipped_intermediate
            ));
        }
        md.push_str(&format!(
            "| Actual total | {} |\n",
            self.summary.actual_total
        ));
        md.push_str(&format!("| Matched | {} |\n", self.summary.matched));
        md.push_str(&format!("| Missing | {} |\n", self.summary.missing));
        md.push_str(&format!("| Unexpected | {} |\n", self.summary.unexpected));
        md.push_str(&format!(
            "| Field mismatch | {} |\n",
            self.summary.field_mismatch
        ));

        // Missing details
        if !self.missing_details.is_empty() {
            md.push_str(&format!(
                "\n### Missing ({})\n\n",
                self.missing_details.len()
            ));
            md.push_str("| Rule | Entity | Score | Time |\n");
            md.push_str("|------|--------|-------|------|\n");
            for d in &self.missing_details {
                md.push_str(&format!(
                    "| {} | {}:{} | {:.2} | {} |\n",
                    d.rule_name, d.entity_type, d.entity_id, d.score, d.time
                ));
            }
        }

        // Unexpected details
        if !self.unexpected_details.is_empty() {
            md.push_str(&format!(
                "\n### Unexpected ({})\n\n",
                self.unexpected_details.len()
            ));
            md.push_str("| Rule | Entity | Score | Time |\n");
            md.push_str("|------|--------|-------|------|\n");
            for d in &self.unexpected_details {
                md.push_str(&format!(
                    "| {} | {}:{} | {:.2} | {} |\n",
                    d.rule_name, d.entity_type, d.entity_id, d.score, d.time
                ));
            }
        }

        // Mismatch details
        if !self.mismatch_details.is_empty() {
            md.push_str(&format!(
                "\n### Field Mismatches ({})\n\n",
                self.mismatch_details.len()
            ));
            md.push_str("| Rule | Entity | Expected | Actual | Exp. Time | Act. Time |\n");
            md.push_str("|------|--------|----------|--------|-----------|----------|\n");
            for d in &self.mismatch_details {
                md.push_str(&format!(
                    "| {} | {}:{} | {:.2} | {:.2} | {} | {} |\n",
                    d.rule_name,
                    d.entity_type,
                    d.entity_id,
                    d.expected_score,
                    d.actual_score,
                    d.expected_time,
                    d.actual_time
                ));
            }
        }

        md
    }
}
