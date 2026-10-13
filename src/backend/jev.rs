//! Jev Choice adapter for topology prompts. No coding-agent pane is spawned.
use super::jev_transport::{self, Failure};
use super::{Backend, Kind, Launch, TopologyInvocation};
use crate::topology::string_enum;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[cfg(test)]
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OnError {
    Fail,
    Defer,
}

impl OnError {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "fail" => Ok(Self::Fail),
            "defer" => Ok(Self::Defer),
            _ => bail!("OMAR_JEV_ON_ERROR must be fail or defer"),
        }
    }
}

pub struct Jev;

impl Backend for Jev {
    fn kind(&self) -> Kind {
        Kind::Jev
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["jev"]
    }
    fn executables(&self) -> &'static [&'static str] {
        &[]
    }
    fn default_command(&self) -> &'static str {
        "jev"
    }
    fn launch_command(&self, _launch: &Launch<'_>) -> String {
        "printf '%s\\n' 'Jev supports topology prompts only; use a Jev agent in an .omar team.' >&2; exit 1".into()
    }
    fn topology_only(&self) -> bool {
        true
    }
    fn invoke_topology(
        &self,
        invocation: &TopologyInvocation<'_>,
    ) -> Result<BTreeMap<String, Value>> {
        invoke(invocation)
    }
}

fn invoke(invocation: &TopologyInvocation<'_>) -> Result<BTreeMap<String, Value>> {
    let TopologyInvocation {
        effects,
        triggers,
        instructions,
        timeout,
    } = invocation;
    let mut decision = None;
    let mut evidence = None;
    for (port, ty) in *effects {
        if let Some(options) = string_enum(ty) {
            ensure!(
                decision.is_none(),
                "Jev requires exactly one string-enum output"
            );
            decision = Some((port, options));
        } else if ty == "string" {
            ensure!(
                evidence.is_none(),
                "Jev accepts at most one JSON evidence string output"
            );
            evidence = Some(port);
        } else {
            bail!("Jev output {port} must be a string enum or JSON evidence string");
        }
    }
    let (port, options) = decision.context("Jev requires one imported string-enum output")?;
    let on_error =
        OnError::parse(&std::env::var("OMAR_JEV_ON_ERROR").unwrap_or_else(|_| "fail".into()))?;
    if on_error == OnError::Defer {
        ensure!(
            options.iter().any(|option| option == "defer") && evidence.is_some(),
            "OMAR_JEV_ON_ERROR=defer requires a defer enum value and an evidence string output"
        );
    }
    let model = std::env::var("OMAR_JEV_MODEL").unwrap_or_else(|_| jev_transport::MODEL.into());
    ensure!(!model.trim().is_empty(), "OMAR_JEV_MODEL must not be empty");
    let criteria: BTreeMap<_, _> = options
        .iter()
        .map(|option| (option.clone(), option.clone()))
        .collect();
    let request = json!({"model": model,
        "state": triggers, "questions": {"decision": {"type": "choice", "instructions": instructions, "criteria": criteria}, "sufficient_context": {"type": "noul", "instructions": "Does the supplied state contain enough evidence to choose a specific permitted outcome?"}}});
    let result =
        jev_transport::evaluate(&request, *timeout, MAX_RESPONSE_BYTES).and_then(|response| {
            let choice = validate(&response, &options, &model).map_err(|_| {
                Failure::new(
                    "invalid_response",
                    "Response does not satisfy the Choice contract",
                )
            })?;
            Ok((choice, response))
        });
    let (choice, mut response) = match result {
        Ok(answer) => answer,
        Err(error) if on_error == OnError::Defer => {
            eprintln!("Jev deferred after {}", error.kind);
            (
                "defer".to_string(),
                json!({"model": model, "error": error.evidence()}),
            )
        }
        Err(error) => return Err(error.into()),
    };
    response["context"] = json!(triggers);
    let answer = &response["answers"]["decision"];
    let eligible = choice != "defer"
        && jev_transport::confident(
            answer["confidence"].as_f64().unwrap_or(0.0),
            answer["probabilities"][&choice].as_f64().unwrap_or(0.0),
            response["answers"]["sufficient_context"]["noul"]
                .as_f64()
                .unwrap_or(0.0),
        );
    response["routing"] = json!({"threshold": jev_transport::THRESHOLD, "eligible": eligible});
    eprintln!("Jev selected {choice}");
    let mut writes = BTreeMap::from([(port.clone(), json!(choice))]);
    if let Some(evidence) = evidence {
        writes.insert(evidence.clone(), json!(response.to_string()));
    }
    Ok(writes)
}

fn validate(response: &Value, options: &[String], model: &str) -> Result<String> {
    ensure!(
        response["model"].as_str() == Some(model),
        "Jev response model does not match the requested model"
    );
    let answer = &response["answers"]["decision"];
    ensure!(answer["type"] == "choice", "Invalid Jev answer type");
    let choice = answer["choice"].as_str().context("Missing Jev choice")?;
    ensure!(
        options.iter().any(|option| option == choice),
        "Unknown Jev choice"
    );
    let confidence = answer["confidence"]
        .as_f64()
        .context("Missing Jev confidence")?;
    ensure!(
        confidence.is_finite() && (0.0..=1.0).contains(&confidence),
        "Invalid confidence"
    );
    let probabilities = answer["probabilities"]
        .as_object()
        .context("Missing probabilities")?;
    ensure!(
        probabilities.len() == options.len(),
        "Incorrect probability options"
    );
    let mut sum = 0.0;
    let mut max: f64 = 0.0;
    for option in options {
        let p = probabilities
            .get(option)
            .and_then(Value::as_f64)
            .context("Missing option probability")?;
        ensure!(
            p.is_finite() && (0.0..=1.0).contains(&p),
            "Invalid probability"
        );
        sum += p;
        max = max.max(p);
    }
    ensure!((sum - 1.0).abs() <= 1e-6, "Probabilities must sum to one");
    ensure!(
        probabilities[choice].as_f64() == Some(max),
        "Choice must have highest probability"
    );
    let context = &response["answers"]["sufficient_context"];
    ensure!(context["type"] == "noul", "Missing Jev context assessment");
    let sufficiency = context["noul"]
        .as_f64()
        .context("Missing context sufficiency")?;
    ensure!(
        sufficiency.is_finite() && (0.0..=1.0).contains(&sufficiency),
        "Invalid context sufficiency"
    );
    Ok(choice.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_output_shapes_fail_before_http() {
        let triggers = BTreeMap::new();
        for effects in [
            BTreeMap::from([("review.evidence".into(), "string".into())]),
            BTreeMap::from([("review.out".into(), "float".into())]),
            BTreeMap::from([
                ("review.first".into(), "string in [\"a\",\"b\"]".into()),
                ("review.second".into(), "string in [\"a\",\"b\"]".into()),
            ]),
        ] {
            let request = TopologyInvocation {
                effects: &effects,
                triggers: &triggers,
                instructions: "not sent",
                timeout: Duration::from_secs(1),
            };
            assert!(Jev.invoke_topology(&request).is_err());
        }
    }

    #[test]
    fn validates_membership_distribution_and_confidence() {
        let options = vec!["proceed".into(), "review".into(), "defer".into()];
        let response = json!({"model":"test-jev","answers":{"decision":{"type":"choice","choice":"proceed","confidence":0.9,
            "probabilities":{"proceed":0.97,"review":0.02,"defer":0.01}}, "sufficient_context":{"type":"noul","noul":0.99}}});
        assert_eq!(
            validate(&response, &options, "test-jev").unwrap(),
            "proceed"
        );
        let mut missing_model = response.clone();
        missing_model["model"] = Value::Null;
        assert!(validate(&missing_model, &options, "test-jev").is_err());
        for (field, value) in [
            ("choice", json!("approve")),
            ("confidence", json!(2)),
            ("confidence", json!(true)),
            ("probabilities", json!({"proceed":0.97,"stop":0.03})),
            (
                "probabilities",
                json!({"proceed":0.97,"review":true,"defer":0.03}),
            ),
            (
                "probabilities",
                json!({"proceed":0.2,"review":0.1,"defer":0.1}),
            ),
        ] {
            let mut bad = response.clone();
            bad["answers"]["decision"][field] = value;
            assert!(validate(&bad, &options, "test-jev").is_err());
        }
    }
}
