use roder_core::FiringAlert;

pub(crate) fn sort_alerts(alerts: &mut [FiringAlert]) {
    alerts.sort_by(|left, right| {
        severity_order(&left.severity)
            .cmp(&severity_order(&right.severity))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.fingerprint.cmp(&right.fingerprint))
    });
}

fn severity_order(severity: &str) -> u8 {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

pub(crate) fn elapsed_since(timestamp: &str) -> String {
    let elapsed = crate::data::humanize_age(&Some(timestamp.to_string()));
    if elapsed.is_empty() {
        timestamp.to_string()
    } else {
        elapsed
    }
}

pub(crate) fn elapsed_since_ms(timestamp: f64) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    return Some(format_elapsed_ms(js_sys::Date::now(), timestamp));

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = timestamp;
        None
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn format_elapsed_ms(now: f64, timestamp: f64) -> String {
    let seconds = ((now - timestamp) / 1000.0).max(0.0) as u64;
    roder_core::format_age_secs(seconds)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn alert(severity: &str, name: &str, fingerprint: &str) -> FiringAlert {
        FiringAlert {
            fingerprint: fingerprint.into(),
            name: name.into(),
            severity: severity.into(),
            summary: String::new(),
            description: String::new(),
            starts_at: String::new(),
            labels: HashMap::new(),
            silenced: false,
        }
    }

    #[test]
    fn alerts_sort_by_severity_then_identity() {
        let mut alerts = vec![
            alert("custom", "zeta", "1"),
            alert("warning", "beta", "2"),
            alert("critical", "zeta", "3"),
            alert("warning", "alpha", "2"),
            alert("warning", "alpha", "1"),
            alert("info", "alpha", "4"),
        ];

        sort_alerts(&mut alerts);

        assert_eq!(
            alerts
                .iter()
                .map(|alert| (
                    alert.severity.as_str(),
                    alert.name.as_str(),
                    alert.fingerprint.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("critical", "zeta", "3"),
                ("warning", "alpha", "1"),
                ("warning", "alpha", "2"),
                ("warning", "beta", "2"),
                ("info", "alpha", "4"),
                ("custom", "zeta", "1"),
            ]
        );
    }

    #[test]
    fn elapsed_milliseconds_are_clamped_and_humanized() {
        assert_eq!(format_elapsed_ms(90_000.0, 0.0), "1m");
        assert_eq!(format_elapsed_ms(1_000.0, 2_000.0), "0s");
    }
}
