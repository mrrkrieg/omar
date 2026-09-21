//! Advisory decision support for Mission Control.
//!
//! This module deliberately has no dependency on the topology control plane.
//! It may observe completed diagram events and persist an operator-requested
//! suggestion, but it cannot write a port, send an agent message, or change a
//! run's lifecycle.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DecisionMode {
    Off,
    Shadow,
    Suggest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Queued,
    Evaluating,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DecisionCoverage {
    Continuous,
    Partial,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DecisionCapabilities {
    pub available: bool,
    pub configured: bool,
    pub model: String,
    pub modes: Vec<DecisionMode>,
    pub max_requests_per_run: u32,
    pub max_source_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DecisionSource {
    pub source_id: String,
    pub run_id: String,
    pub reaction_id: String,
    pub port: String,
    pub sha256: String,
    pub captured_at: i64,
    pub coverage: DecisionCoverage,
    /// Kept on the loopback-only local API so the operator can select the
    /// exact excerpt to evaluate. It is never sent until explicitly selected.
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DecisionRecord {
    pub decision_id: String,
    pub request_id: String,
    pub run_id: String,
    pub source_id: String,
    pub source_sha256: String,
    pub profile_id: String,
    pub mode: DecisionMode,
    pub status: DecisionStatus,
    pub coverage: DecisionCoverage,
    pub freshness: String,
    pub reason_code: String,
    pub suggestion: String,
    pub confidence: f64,
    pub selected_probability: f64,
    pub model: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EvaluateRequest {
    pub request_id: String,
    pub profile_id: String,
    pub source_id: String,
    pub source_sha256: String,
    pub selection_start: usize,
    pub selection_end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackRequest {
    pub request_id: String,
    pub outcome: String,
    #[serde(default)]
    pub note: String,
}

#[cfg(feature = "decision-support")]
mod enabled {
    use super::*;
    use crate::config::DecisionSupportConfig;
    use anyhow::{anyhow, Context, Result};
    use reqwest::blocking::Client;
    use reqwest::redirect::Policy;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::io::{BufRead, BufReader};
    use std::net::{SocketAddr, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use uuid::Uuid;

    const MAX_REQUESTS_PER_RUN: u32 = 100;
    const MAX_SOURCE_BYTES: usize = 64 * 1024;
    const MAX_SELECTION_BYTES: usize = 16 * 1024;
    const QUEUE_DEPTH: usize = 32;

    #[derive(Debug, Clone, Deserialize)]
    struct ProviderAnswer {
        question_id: String,
        #[serde(default)]
        selected: Option<String>,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
    }

    #[derive(Debug, Clone, Deserialize)]
    struct ProviderResponse {
        model: String,
        answers: Vec<ProviderAnswer>,
    }

    pub trait Provider: Send + Sync {
        fn evaluate(&self, selected: &str) -> Result<ProviderResponse>;
    }

    struct TypeSafeProvider {
        client: Client,
        endpoint: String,
        model: String,
    }

    impl TypeSafeProvider {
        fn new(config: &DecisionSupportConfig) -> Result<Self> {
            let client = Client::builder()
                .timeout(Duration::from_secs(config.timeout_seconds.clamp(1, 3)))
                .redirect(Policy::none())
                .build()
                .context("build Jev client")?;
            Ok(Self {
                client,
                endpoint: format!(
                    "{}/v1/systemone",
                    config.typesafe_base_url.trim_end_matches('/')
                ),
                model: config.model.clone(),
            })
        }
    }

    impl Provider for TypeSafeProvider {
        fn evaluate(&self, selected: &str) -> Result<ProviderResponse> {
            let key = std::env::var("TYPESAFE_API_KEY")
                .map_err(|_| anyhow!("TYPESAFE_API_KEY is not configured"))?;
            let response = self
                .client
                .post(&self.endpoint)
                .bearer_auth(key)
                .json(&json!({
                    "model": self.model,
                    "input": {"source": selected},
                    "questions": [
                        {"id": "owner", "type": "single_choice", "choices": ["backend", "frontend", "design", "qa", "multiple", "uncertain"]},
                        {"id": "sufficient_context", "type": "boolean"}
                    ]
                }))
                .send()
                .context("call TypeSafe SystemOne")?
                .error_for_status()
                .context("TypeSafe SystemOne rejected request")?;
            response.json().context("parse TypeSafe SystemOne response")
        }
    }

    #[derive(Default)]
    struct RunState {
        mode: DecisionMode,
        coverage: DecisionCoverage,
        observer_attached: bool,
        sources: BTreeMap<String, DecisionSource>,
        decisions: BTreeMap<String, DecisionRecord>,
        requests: BTreeMap<String, String>,
        feedback: BTreeMap<String, FeedbackRequest>,
    }

    struct State {
        runs: BTreeMap<String, RunState>,
        sender: Option<mpsc::SyncSender<Job>>,
        workers_started: bool,
    }

    impl Default for DecisionMode {
        fn default() -> Self {
            Self::Off
        }
    }
    impl Default for DecisionCoverage {
        fn default() -> Self {
            Self::Continuous
        }
    }

    struct Job {
        run_id: String,
        decision_id: String,
        selected: String,
    }

    pub struct DecisionService {
        config: DecisionSupportConfig,
        root: PathBuf,
        provider: Arc<dyn Provider>,
        state: Arc<Mutex<State>>,
    }

    impl DecisionService {
        pub fn new(config: DecisionSupportConfig, omar_dir: &Path) -> Result<Self> {
            let provider = Arc::new(TypeSafeProvider::new(&config)?);
            Ok(Self::with_provider(config, omar_dir, provider))
        }

        #[cfg(test)]
        fn with_test_provider(
            config: DecisionSupportConfig,
            omar_dir: &Path,
            provider: Arc<dyn Provider>,
        ) -> Self {
            Self::with_provider(config, omar_dir, provider)
        }

        fn with_provider(
            config: DecisionSupportConfig,
            omar_dir: &Path,
            provider: Arc<dyn Provider>,
        ) -> Self {
            Self {
                config,
                root: omar_dir.join("decisions"),
                provider,
                state: Arc::new(Mutex::new(State {
                    runs: BTreeMap::new(),
                    sender: None,
                    workers_started: false,
                })),
            }
        }

        pub fn capabilities(&self) -> DecisionCapabilities {
            DecisionCapabilities {
                available: true,
                configured: self.config.enabled,
                model: self.config.model.clone(),
                modes: vec![
                    DecisionMode::Off,
                    DecisionMode::Shadow,
                    DecisionMode::Suggest,
                ],
                max_requests_per_run: MAX_REQUESTS_PER_RUN,
                max_source_bytes: MAX_SOURCE_BYTES as u32,
            }
        }

        pub fn set_mode(&self, run_id: &str, mode: DecisionMode) -> Result<()> {
            if !self.config.enabled {
                anyhow::bail!("decision support is disabled in config")
            }
            let mut state = self.state.lock().expect("decision support poisoned");
            let run = state.runs.entry(run_id.to_string()).or_default();
            run.mode = mode;
            if mode != DecisionMode::Off {
                self.start_workers_locked(&mut state);
            }
            Ok(())
        }

        /// Attach to the daemon-issued diagram address. This is intentionally a
        /// client of the existing loopback SSE stream, not a second callback in
        /// the topology runtime: a failed observer cannot delay a reaction.
        pub fn attach_observer(&self, run_id: String, address: SocketAddr) {
            if !address.ip().is_loopback() {
                return;
            }
            {
                let mut state = self.state.lock().expect("decision support poisoned");
                let run = state.runs.entry(run_id.clone()).or_default();
                if run.mode == DecisionMode::Off || run.observer_attached {
                    return;
                }
                run.observer_attached = true;
            }
            let service = self.clone_for_worker();
            thread::spawn(move || {
                let result = service.observe(&run_id, address);
                if result.is_err() {
                    service.mark_coverage(&run_id, DecisionCoverage::Partial);
                }
            });
        }

        pub fn capture(
            &self,
            run_id: &str,
            reaction_id: &str,
            port: &str,
            text: &str,
        ) -> Result<Option<DecisionSource>> {
            let mut state = self.state.lock().expect("decision support poisoned");
            let run = state.runs.entry(run_id.to_string()).or_default();
            if run.mode == DecisionMode::Off || !is_review_output(reaction_id, port) {
                return Ok(None);
            }
            let text = truncate_utf8(text, MAX_SOURCE_BYTES);
            let source = DecisionSource {
                source_id: Uuid::new_v4().to_string(),
                run_id: run_id.to_string(),
                reaction_id: reaction_id.to_string(),
                port: port.to_string(),
                sha256: sha256(&text),
                captured_at: now_unix(),
                coverage: run.coverage,
                text,
            };
            run.sources.insert(source.source_id.clone(), source.clone());
            drop(state);
            self.persist(run_id, "source", &source.source_id, &source)?;
            Ok(Some(source))
        }

        pub fn mark_coverage(&self, run_id: &str, coverage: DecisionCoverage) {
            let mut state = self.state.lock().expect("decision support poisoned");
            state.runs.entry(run_id.to_string()).or_default().coverage = coverage;
        }

        pub fn sources(&self, run_id: &str) -> (Vec<DecisionSource>, DecisionCoverage) {
            let state = self.state.lock().expect("decision support poisoned");
            let Some(run) = state.runs.get(run_id) else {
                return (Vec::new(), DecisionCoverage::Stale);
            };
            (run.sources.values().cloned().collect(), run.coverage)
        }

        pub fn decisions(&self, run_id: &str) -> (Vec<DecisionRecord>, DecisionCoverage) {
            let state = self.state.lock().expect("decision support poisoned");
            let Some(run) = state.runs.get(run_id) else {
                return (Vec::new(), DecisionCoverage::Stale);
            };
            (run.decisions.values().cloned().collect(), run.coverage)
        }

        pub fn evaluate(&self, run_id: &str, request: EvaluateRequest) -> Result<DecisionRecord> {
            if request.request_id.trim().is_empty() {
                anyhow::bail!("request_id is required")
            }
            let mut state = self.state.lock().expect("decision support poisoned");
            let run = state.runs.entry(run_id.to_string()).or_default();
            if run.mode != DecisionMode::Suggest {
                anyhow::bail!("suggestions are not active for this run")
            }
            if let Some(id) = run.requests.get(&request.request_id) {
                return run
                    .decisions
                    .get(id)
                    .cloned()
                    .ok_or_else(|| anyhow!("idempotency record missing"));
            }
            if run.requests.len() >= MAX_REQUESTS_PER_RUN as usize {
                anyhow::bail!("request limit reached for this run")
            }
            let source = run
                .sources
                .get(&request.source_id)
                .ok_or_else(|| anyhow!("unknown source"))?;
            if source.sha256 != request.source_sha256 {
                anyhow::bail!("source digest does not match")
            }
            let selected =
                unicode_slice(&source.text, request.selection_start, request.selection_end)?;
            if selected.as_bytes().len() > MAX_SELECTION_BYTES {
                anyhow::bail!("selection is too large")
            }
            if request.profile_id != "review-owner-v1" {
                anyhow::bail!("unknown decision profile")
            }
            let record = DecisionRecord {
                decision_id: Uuid::new_v4().to_string(),
                request_id: request.request_id.clone(),
                run_id: run_id.to_string(),
                source_id: source.source_id.clone(),
                source_sha256: source.sha256.clone(),
                profile_id: request.profile_id,
                mode: run.mode,
                status: DecisionStatus::Queued,
                coverage: run.coverage,
                freshness: "fresh".to_string(),
                reason_code: "queued".to_string(),
                suggestion: "needs_review".to_string(),
                confidence: 0.0,
                selected_probability: 0.0,
                model: None,
                created_at: now_unix(),
                completed_at: None,
                error: None,
            };
            run.requests
                .insert(record.request_id.clone(), record.decision_id.clone());
            run.decisions
                .insert(record.decision_id.clone(), record.clone());
            self.start_workers_locked(&mut state);
            let sender = state.sender.as_ref().expect("workers started").clone();
            drop(state);
            self.persist(run_id, "decision", &record.decision_id, &record)?;
            sender
                .try_send(Job {
                    run_id: run_id.to_string(),
                    decision_id: record.decision_id.clone(),
                    selected,
                })
                .map_err(|_| anyhow!("decision queue is full"))?;
            Ok(record)
        }

        pub fn feedback(
            &self,
            run_id: &str,
            decision_id: &str,
            feedback: FeedbackRequest,
        ) -> Result<()> {
            if feedback.request_id.trim().is_empty() {
                anyhow::bail!("request_id is required")
            }
            let mut state = self.state.lock().expect("decision support poisoned");
            let run = state
                .runs
                .get_mut(run_id)
                .ok_or_else(|| anyhow!("unknown run"))?;
            if !run.decisions.contains_key(decision_id) {
                anyhow::bail!("unknown decision")
            }
            if let Some(existing) = run.feedback.get(&feedback.request_id) {
                if existing.outcome != feedback.outcome || existing.note != feedback.note {
                    anyhow::bail!("request_id already used")
                }
                return Ok(());
            }
            if !matches!(
                feedback.outcome.as_str(),
                "accepted" | "rejected" | "corrected"
            ) {
                anyhow::bail!("invalid feedback outcome")
            }
            run.feedback
                .insert(feedback.request_id.clone(), feedback.clone());
            drop(state);
            self.persist(run_id, "feedback", decision_id, &feedback)
        }

        fn start_workers_locked(&self, state: &mut State) {
            if state.workers_started {
                return;
            }
            let (sender, receiver) = mpsc::sync_channel(QUEUE_DEPTH);
            let receiver = Arc::new(Mutex::new(receiver));
            for _ in 0..2 {
                let receiver = receiver.clone();
                let service = self.clone_for_worker();
                thread::spawn(move || loop {
                    let job = match receiver.lock().expect("decision queue poisoned").recv() {
                        Ok(job) => job,
                        Err(_) => break,
                    };
                    service.complete(job);
                });
            }
            state.sender = Some(sender);
            state.workers_started = true;
        }

        fn clone_for_worker(&self) -> Self {
            Self {
                config: self.config.clone(),
                root: self.root.clone(),
                provider: self.provider.clone(),
                state: self.state.clone(),
            }
        }

        fn complete(&self, job: Job) {
            {
                let mut state = self.state.lock().expect("decision support poisoned");
                if let Some(record) = state
                    .runs
                    .get_mut(&job.run_id)
                    .and_then(|run| run.decisions.get_mut(&job.decision_id))
                {
                    record.status = DecisionStatus::Evaluating;
                } else {
                    return;
                }
            }
            let result = self
                .provider
                .evaluate(&job.selected)
                .and_then(validate_response)
                .map(policy);
            let record = {
                let mut state = self.state.lock().expect("decision support poisoned");
                let Some(record) = state
                    .runs
                    .get_mut(&job.run_id)
                    .and_then(|run| run.decisions.get_mut(&job.decision_id))
                else {
                    return;
                };
                match result {
                    Ok(outcome) => {
                        record.status = DecisionStatus::Ready;
                        record.suggestion = outcome.suggestion;
                        record.confidence = outcome.confidence;
                        record.selected_probability = outcome.selected_probability;
                        record.reason_code = outcome.reason_code;
                        record.model = Some(outcome.model);
                    }
                    Err(error) => {
                        record.status = DecisionStatus::Failed;
                        record.reason_code = "provider_failure".to_string();
                        record.error = Some(error.to_string());
                    }
                }
                record.completed_at = Some(now_unix());
                record.clone()
            };
            let _ = self.persist(&job.run_id, "decision", &record.decision_id, &record);
        }

        fn observe(&self, run_id: &str, address: SocketAddr) -> Result<()> {
            let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
            stream.set_read_timeout(Some(Duration::from_secs(15)))?;
            write!(stream, "GET /v1/events HTTP/1.1\r\nHost: {address}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n")?;
            stream.flush()?;
            let mut reader = BufReader::new(stream);
            let mut status = String::new();
            reader.read_line(&mut status)?;
            if !status.contains(" 200 ") {
                anyhow::bail!("diagram stream refused observer")
            }
            loop {
                let mut line = String::new();
                reader.read_line(&mut line)?;
                if line == "\r\n" {
                    break;
                }
            }
            let mut event = String::new();
            let mut data = String::new();
            let mut last_sequence = 0u64;
            loop {
                let mut line = String::new();
                let read = reader.read_line(&mut line)?;
                if read == 0 {
                    break;
                }
                if line.len() > 70 * 1024 {
                    anyhow::bail!("diagram event exceeded observer bound")
                }
                if let Some(value) = line.strip_prefix("event:") {
                    event = value.trim().to_string();
                    continue;
                }
                if let Some(value) = line.strip_prefix("data:") {
                    data.push_str(value.trim());
                    continue;
                }
                if line == "\n" || line == "\r\n" {
                    if event == "reaction_completed" && !data.is_empty() {
                        let value: Value = serde_json::from_str(&data)?;
                        let sequence = value.get("sequence").and_then(Value::as_u64).unwrap_or(0);
                        if last_sequence != 0 && sequence != last_sequence + 1 {
                            self.mark_coverage(run_id, DecisionCoverage::Partial);
                        }
                        last_sequence = sequence;
                        let reaction = value
                            .pointer("/payload/reaction")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if let Some(writes) =
                            value.pointer("/payload/writes").and_then(Value::as_object)
                        {
                            for (port, output) in writes {
                                if let Some(text) = output.as_str() {
                                    let _ = self.capture(run_id, reaction, port, text);
                                }
                            }
                        }
                    }
                    event.clear();
                    data.clear();
                }
            }
            Ok(())
        }

        fn persist<T: Serialize>(
            &self,
            run_id: &str,
            kind: &str,
            id: &str,
            value: &T,
        ) -> Result<()> {
            let directory = self.root.join(run_id);
            write_private(
                &directory,
                &format!("{kind}-{id}.json"),
                &serde_json::to_vec(value)?,
            )
        }
    }

    struct PolicyOutcome {
        suggestion: String,
        confidence: f64,
        selected_probability: f64,
        reason_code: String,
        model: String,
    }

    fn validate_response(response: ProviderResponse) -> Result<ProviderResponse> {
        if response.model != "jev-1.13.0" {
            anyhow::bail!("unexpected model resolution")
        }
        if response.answers.len() != 2 {
            anyhow::bail!("incomplete Jev response")
        }
        let mut found = BTreeMap::new();
        for answer in &response.answers {
            if !matches!(answer.question_id.as_str(), "owner" | "sufficient_context")
                || found.insert(answer.question_id.clone(), answer).is_some()
            {
                anyhow::bail!("unexpected question id")
            }
            if answer.probabilities.is_empty()
                || answer
                    .probabilities
                    .values()
                    .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
            {
                anyhow::bail!("invalid probability")
            }
            let sum: f64 = answer.probabilities.values().sum();
            if (sum - 1.0).abs() > 0.001 {
                anyhow::bail!("probabilities do not sum to one")
            }
        }
        let owner = found.get("owner").unwrap();
        let Some(choice) = owner.selected.as_deref() else {
            anyhow::bail!("owner choice missing")
        };
        if !matches!(
            choice,
            "backend" | "frontend" | "design" | "qa" | "multiple" | "uncertain"
        ) || !owner.probabilities.contains_key(choice)
        {
            anyhow::bail!("invalid owner choice")
        }
        let context = found.get("sufficient_context").unwrap();
        if context.selected.as_deref() != Some("true")
            || !context.probabilities.contains_key("true")
        {
            anyhow::bail!("invalid sufficient_context answer")
        }
        Ok(response)
    }

    fn policy(response: ProviderResponse) -> PolicyOutcome {
        let owner = response
            .answers
            .iter()
            .find(|answer| answer.question_id == "owner")
            .unwrap();
        let context = response
            .answers
            .iter()
            .find(|answer| answer.question_id == "sufficient_context")
            .unwrap();
        let choice = owner.selected.as_deref().unwrap();
        let selected_probability = owner.probabilities[choice];
        let confidence = context.probabilities["true"];
        let specific = matches!(choice, "backend" | "frontend" | "design" | "qa");
        let qualifies = specific && selected_probability >= 0.90 && confidence >= 0.90;
        PolicyOutcome {
            suggestion: if qualifies {
                choice.to_string()
            } else {
                "needs_review".to_string()
            },
            confidence,
            selected_probability,
            reason_code: if qualifies {
                "high_confidence_owner".to_string()
            } else {
                "insufficient_confidence".to_string()
            },
            model: response.model,
        }
    }

    fn is_review_output(reaction_id: &str, port: &str) -> bool {
        reaction_id.contains("review") && port == "review"
    }
    fn now_unix() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }
    fn sha256(text: &str) -> String {
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }
    fn truncate_utf8(text: &str, max: usize) -> String {
        if text.len() <= max {
            text.to_string()
        } else {
            text.char_indices()
                .take_while(|(index, _)| *index < max)
                .map(|(_, c)| c)
                .collect()
        }
    }
    fn unicode_slice(text: &str, start: usize, end: usize) -> Result<String> {
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        if start >= end || end > chars.len() {
            anyhow::bail!("selection range is outside source")
        };
        let begin = chars[start].0;
        let finish = chars.get(end).map(|(i, _)| *i).unwrap_or(text.len());
        Ok(text[begin..finish].to_string())
    }

    fn write_private(directory: &Path, filename: &str, bytes: &[u8]) -> Result<()> {
        fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let temporary = directory.join(format!(".{filename}.{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, directory.join(filename))?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn unicode_ranges_are_scalar_offsets() {
            assert_eq!(unicode_slice("aé🙂z", 1, 3).unwrap(), "é🙂");
        }
        #[test]
        fn conservative_policy_needs_three_high_confidence_signals() {
            let response = ProviderResponse {
                model: "jev-1.13.0".to_string(),
                answers: vec![ProviderAnswer { question_id: "owner".to_string(), selected: Some("backend".to_string()), probabilities: BTreeMap::from([("backend".to_string(), 0.91), ("frontend".to_string(), 0.09)]) }, ProviderAnswer { question_id: "sufficient_context".to_string(), selected: Some("true".to_string()), probabilities: BTreeMap::from([("true".to_string(), 0.89), ("false".to_string(), 0.11)]) }],
            };
            assert_eq!(
                policy(validate_response(response).unwrap()).suggestion,
                "needs_review"
            );
        }
    }

    pub use DecisionService as Service;
}

#[cfg(feature = "decision-support")]
pub use enabled::Service;
