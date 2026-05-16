use std::collections::HashMap;

use serde_json::{json, Value};

use crate::models::alert::{
    AlertCondition, AlertSeverity, PresenceExpectation, ThresholdOp, WidgetAlert,
};
use crate::modules::workflow_engine::resolve_path;

/// One alert that just fired. The caller persists this into
/// `alert_events`, emits an event, and (for autonomous triggers) spawns
/// a chat session.
#[derive(Debug, Clone)]
pub struct FiredAlert {
    pub alert: WidgetAlert,
    pub severity: AlertSeverity,
    pub message: String,
    pub context: Value,
}

/// Evaluate every alert against `data`. `last_fired_at` provides the
/// last `fired_at` per alert id so the cooldown can be respected
/// without a fresh DB round-trip per alert.
pub fn evaluate(
    alerts: &[WidgetAlert],
    data: &Value,
    last_fired_at: &HashMap<String, i64>,
    now_ms: i64,
) -> Vec<FiredAlert> {
    let mut out = Vec::new();
    for alert in alerts {
        if !alert.enabled {
            continue;
        }
        if let Some(last) = last_fired_at.get(&alert.id) {
            let cooldown_ms = (alert.cooldown_seconds as i64).saturating_mul(1000);
            if now_ms - last < cooldown_ms {
                continue;
            }
        }
        if let Some(fired) = evaluate_one(alert, data) {
            out.push(fired);
        }
    }
    out
}

/// Pure single-condition evaluation. Public for the
/// `test_alert_condition` preview command.
pub fn test_condition(condition: &AlertCondition, data: &Value) -> (bool, Value, Option<String>) {
    let value_for_path = |path: &str| resolve_path(data, path);
    match condition {
        AlertCondition::Threshold { path, op, value } => {
            let resolved = value_for_path(path);
            let fired = compare_threshold(&resolved, *op, value);
            (fired, resolved, None)
        }
        AlertCondition::PathPresent { path, expected } => {
            let resolved = value_for_path(path);
            let fired = check_presence(&resolved, *expected);
            (fired, resolved, None)
        }
        AlertCondition::StatusEquals { path, status } => {
            let resolved = value_for_path(path);
            let fired = matches_status(&resolved, status);
            (fired, resolved, None)
        }
        AlertCondition::Custom { jmespath_expr } => {
            let resolved = value_for_path(jmespath_expr);
            let fired = is_truthy(&resolved);
            (
                fired,
                resolved,
                Some("custom v1 evaluates jmespath_expr as resolve_path truthiness".into()),
            )
        }
    }
}

fn evaluate_one(alert: &WidgetAlert, data: &Value) -> Option<FiredAlert> {
    let (fired, resolved, _) = test_condition(&alert.condition, data);
    if !fired {
        return None;
    }
    let (path, threshold) = match &alert.condition {
        AlertCondition::Threshold { path, value, .. } => (path.clone(), value.clone()),
        AlertCondition::PathPresent { path, .. } => (path.clone(), Value::Null),
        AlertCondition::StatusEquals { path, status } => {
            (path.clone(), Value::String(status.clone()))
        }
        AlertCondition::Custom { jmespath_expr } => (jmespath_expr.clone(), Value::Null),
    };
    let context = json!({
        "value": resolved,
        "path": path,
        "threshold": threshold,
    });
    let message = render_message(&alert.message_template, &context);
    Some(FiredAlert {
        alert: alert.clone(),
        severity: alert.severity,
        message,
        context,
    })
}

fn compare_threshold(resolved: &Value, op: ThresholdOp, threshold: &Value) -> bool {
    let lhs = as_f64(resolved);
    let rhs = as_f64(threshold);
    match (lhs, rhs, op) {
        (Some(a), Some(b), ThresholdOp::Gt) => a > b,
        (Some(a), Some(b), ThresholdOp::Lt) => a < b,
        (Some(a), Some(b), ThresholdOp::Gte) => a >= b,
        (Some(a), Some(b), ThresholdOp::Lte) => a <= b,
        (Some(a), Some(b), ThresholdOp::Eq) => (a - b).abs() < f64::EPSILON,
        (Some(a), Some(b), ThresholdOp::Neq) => (a - b).abs() >= f64::EPSILON,
        // Fallback: string/value-level equality for non-numeric domains.
        (None, _, ThresholdOp::Eq) | (_, None, ThresholdOp::Eq) => resolved == threshold,
        (None, _, ThresholdOp::Neq) | (_, None, ThresholdOp::Neq) => resolved != threshold,
        _ => false,
    }
}

fn check_presence(resolved: &Value, expected: PresenceExpectation) -> bool {
    let is_present = !matches!(resolved, Value::Null);
    let is_empty = match resolved {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    };
    match expected {
        PresenceExpectation::Present => is_present,
        PresenceExpectation::Absent => !is_present,
        PresenceExpectation::Empty => is_empty,
        PresenceExpectation::NonEmpty => !is_empty,
    }
}

fn matches_status(resolved: &Value, status: &str) -> bool {
    match resolved {
        Value::String(s) => s.eq_ignore_ascii_case(status),
        Value::Bool(b) => b.to_string().eq_ignore_ascii_case(status),
        Value::Number(n) => n.to_string() == status,
        _ => false,
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|v| v != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn render_message(template: &str, context: &Value) -> String {
    let mut out = template.to_string();
    for key in ["value", "path", "threshold"] {
        let placeholder = format!("{{{}}}", key);
        if !out.contains(&placeholder) {
            continue;
        }
        let rendered = match context.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        out = out.replace(&placeholder, &rendered);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::alert::{AlertSeverity, ThresholdOp};

    fn alert(condition: AlertCondition) -> WidgetAlert {
        WidgetAlert {
            id: "a".into(),
            name: "test".into(),
            condition,
            severity: AlertSeverity::Warning,
            message_template: "value={value} path={path} threshold={threshold}".into(),
            cooldown_seconds: 60,
            enabled: true,
            agent_action: None,
        }
    }

    #[test]
    fn threshold_gt_fires_when_value_exceeds() {
        let a = alert(AlertCondition::Threshold {
            path: "metric.value".into(),
            op: ThresholdOp::Gt,
            value: json!(50),
        });
        let data = json!({"metric": {"value": 92}});
        let fired = evaluate(&[a], &data, &HashMap::new(), 0);
        assert_eq!(fired.len(), 1);
        assert!(fired[0].message.contains("value=92"));
    }

    #[test]
    fn cooldown_suppresses_repeat_firing() {
        let a = alert(AlertCondition::Threshold {
            path: "v".into(),
            op: ThresholdOp::Gt,
            value: json!(1),
        });
        let data = json!({"v": 10});
        let mut last = HashMap::new();
        last.insert("a".to_string(), 1_000);
        let fired = evaluate(&[a], &data, &last, 30_000);
        assert!(fired.is_empty(), "still within 60s cooldown");
    }

    #[test]
    fn presence_absent_matches_missing_path() {
        let a = alert(AlertCondition::PathPresent {
            path: "missing.field".into(),
            expected: PresenceExpectation::Absent,
        });
        let data = json!({"other": 1});
        let fired = evaluate(&[a], &data, &HashMap::new(), 0);
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn status_equals_is_case_insensitive() {
        let a = alert(AlertCondition::StatusEquals {
            path: "status".into(),
            status: "DOWN".into(),
        });
        let data = json!({"status": "down"});
        let fired = evaluate(&[a], &data, &HashMap::new(), 0);
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn disabled_alert_does_not_fire() {
        let mut a = alert(AlertCondition::Threshold {
            path: "v".into(),
            op: ThresholdOp::Gt,
            value: json!(0),
        });
        a.enabled = false;
        let data = json!({"v": 100});
        let fired = evaluate(&[a], &data, &HashMap::new(), 0);
        assert!(fired.is_empty());
    }
}
