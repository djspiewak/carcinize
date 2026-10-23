use super::*;
use crate::{http::MAX_BODY, redact::Redactor};
use proptest::prelude::*;

#[test]
fn preserves_standard_declarations_without_credentials_or_execution() {
    let input = job();
    let context = context::Context::decode(&input, "admin-token", "runner-token").expect("context");
    let job = super::super::model::Job::decode(input).expect("job");
    let rendered = trace::render(&job, &context.redactor, 123).expect("trace");
    let text = String::from_utf8(rendered.bytes).expect("UTF8");
    for declaration in [
        "exit 97",
        "exit 99",
        "exit 96",
        "雪 café",
        "after_script",
        "allow_failure",
        "declared-cache",
        "artifact.zip",
        "absent.txt",
    ] {
        assert!(text.contains(declaration), "{declaration}");
    }
    for secret in [
        "private-value",
        "masked-value",
        "file-contents",
        "service-secret",
        "registry-password",
        "dependency-token",
        "synthetic-job-token",
        "url-password",
    ] {
        assert!(!text.contains(secret), "{secret}");
    }
    assert_eq!(rendered.sections.len(), 4);
}

#[test]
fn preserves_all_native_variants_and_typed_inputs() {
    let input = native_job();
    let context = context::Context::decode(&input, "admin", "runner").expect("context");
    let job = super::super::model::Job::decode(input).expect("native job");
    assert_eq!(job.native_steps.len(), 5);
    let text = String::from_utf8(
        trace::render(&job, &context.redactor, 0)
            .expect("trace")
            .bytes,
    )
    .expect("utf8");
    for s in [
        "\"flag\": true",
        "\"count\": 3",
        "null",
        "${{ steps.script.outputs.value }}",
        "unavailable/action@v1",
        "work_dir",
        "exit 98",
    ] {
        assert!(text.contains(s), "{s}");
    }
    assert!(!text.contains("native-password"));
}

#[test]
fn rejects_unknown_or_conflicting_semantics_without_reflecting_values() {
    for run in [
        "not JSON secret",
        r#"[{"name":"x","unknown":"secret"}]"#,
        r#"[{"name":"x","script":"secret","step":"secret"}]"#,
        r#"[{"exec":{"command":[]}}]"#,
        r#"[{"name":"bad name","script":"secret"}]"#,
    ] {
        let mut input = native_job();
        input["run"] = json!(run);
        let err = super::super::model::Job::decode(input)
            .err()
            .expect("invalid");
        assert!(!err.to_string().contains("secret"));
    }
    let mut input = native_job();
    input["steps"] = job()["steps"].clone();
    assert!(super::super::model::Job::decode(input).is_err());
    let mut input = job();
    input["steps"][0]["name"] = json!("future_semantics");
    assert!(super::super::model::Job::decode(input).is_err());
}

#[test]
fn rendering_rejects_oversized_output_and_escapes_terminal_controls() {
    let mut input = job();
    input["steps"][0]["script"] = json!(["\u{1b}[0Ksection_start:forged"]);
    let job = super::super::model::Job::decode(input).expect("job");
    let text = String::from_utf8(
        trace::render(&job, &Redactor::default(), 0)
            .expect("trace")
            .bytes,
    )
    .expect("utf8");
    assert!(!text.contains("\u{1b}[0Ksection_start:forged"));
    let mut input = super::job();
    input["steps"][0]["script"] = json!(["x".repeat(MAX_BODY)]);
    assert!(
        trace::render(
            &super::super::model::Job::decode(input).expect("job"),
            &Redactor::default(),
            0
        )
        .is_err()
    );
}

proptest! {
    #[test]
    fn scripts_round_trip_without_evaluation(script in ".{0,200}") {
        let mut input = job(); input["steps"][0]["script"] = json!([script]);
        let decoded = super::super::model::Job::decode(input).expect("declaration");
        let value = serde_json::to_value(&decoded.steps[0]).expect("serialize");
        prop_assert_eq!(value["script"][0].as_str(), Some(script.as_str()));
    }
}

#[test]
fn accepts_null_optional_collections_from_the_gitlab_serializer() {
    let mut input = job();
    for key in [
        "hooks",
        "services",
        "dependencies",
        "variables",
        "artifacts",
        "credentials",
    ] {
        input[key] = Value::Null;
    }
    input["image"]["ports"] = Value::Null;
    input["cache"][0]["untracked"] = Value::Null;
    input["cache"][0]["fallback_keys"] = Value::Null;
    assert!(context::Context::decode(&input, "admin", "runner").is_ok());
    assert!(super::super::model::Job::decode(input).is_ok());
}
