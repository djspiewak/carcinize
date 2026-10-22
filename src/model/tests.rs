use super::*;
use serde_json::json;

pub(crate) fn script_step() -> serde_json::Value {
    json!({"type":4,"id":"11111111-1111-4111-8111-111111111111","contextName":"inspect_me",
        "displayNameToken":{"type":0,"lit":"A script","file":1,"line":3,"col":5},
        "condition":"success() && false", "continueOnError":true, "timeoutInMinutes":2,
        "reference":{"type":3}, "environment":{"type":2,"map":[{"Key":"EXAMPLE","Value":{"type":3,"expr":"github.ref"}}]},
        "inputs":{"type":2,"map":[{"Key":"script","Value":"echo hello\nexit 97\n"},
            {"Key":"shell","Value":"bash"},{"Key":"workingDirectory","Value":"missing-directory"}]}})
}

#[test]
fn decodes_script_without_evaluating_its_contents_or_condition() {
    let step = decode_step(script_step()).expect("valid script");
    assert_eq!(step.context_name.as_deref(), Some("inspect_me"));
    assert_eq!(step.condition.as_deref(), Some("success() && false"));
    let StepKind::Script {
        script,
        shell,
        working_directory,
    } = &step.kind
    else {
        panic!("script kind")
    };
    assert_eq!(script.as_literal(), Some("echo hello\nexit 97\n"));
    assert_eq!(
        shell.as_ref().and_then(TemplateValue::as_literal),
        Some("bash")
    );
    assert_eq!(
        working_directory
            .as_ref()
            .and_then(TemplateValue::as_literal),
        Some("missing-directory")
    );
    assert_eq!(
        step.display_name.as_ref().expect("name").source.line,
        Some(3)
    );
    let encoded = serde_json::to_string(&step).expect("serialize");
    assert!(encoded.contains("github.ref"));
    assert!(encoded.contains("expression"));
}

#[test]
fn distinguishes_repository_local_and_container_actions() {
    for (reference, expected) in [
        (
            json!({"type":1,"repositoryType":"GitHub","name":"actions/checkout","ref":"v4","path":""}),
            "repository_action",
        ),
        (
            json!({"type":1,"repositoryType":"self","path":"./.github/actions/example"}),
            "local_action",
        ),
        (json!({"type":2,"image":"alpine:3"}), "container_action"),
    ] {
        let mut wire = script_step();
        wire["reference"] = reference;
        let step = decode_step(wire).expect("action");
        assert_eq!(
            serde_json::to_value(step.kind).expect("JSON")["kind"],
            expected
        );
    }
}

#[test]
fn preserves_expression_keys_insertions_and_scalar_sources() {
    let value = decode_template(json!({"type":2,"map":[
        {"Key":{"type":4,"directive":"insert"},"Value":{"type":3,"expr":"inputs.env"}},
        {"Key":"b","Value":{"type":5,"bool":true,"line":4}},
        {"Key":"n","Value":{"type":6,"num":2.5}},
        {"Key":"empty","Value":{"type":7}},
        {"Key":"list","Value":{"type":1,"seq":[false,null,1,"text"]}}
    ]}))
    .expect("template");
    let rendered = serde_json::to_string(&value).expect("JSON");
    assert!(rendered.contains("insert"));
    assert!(rendered.contains("inputs.env"));
    assert!(rendered.contains("2.5"));
}

#[test]
fn rejects_unknown_semantics_and_missing_script() {
    assert!(decode_template(json!({"type":99,"lit":"do not flatten"})).is_err());
    let mut step = script_step();
    step["type"] = json!(5);
    assert!(decode_step(step.clone()).is_err());
    step["type"] = json!(4);
    step["reference"]["type"] = json!(99);
    assert!(decode_step(step.clone()).is_err());
    step["reference"]["type"] = json!(3);
    step["inputs"] = json!({"type":2,"map":[]});
    assert!(decode_step(step).is_err());
}

#[test]
fn preserves_job_order_environment_and_defaults() {
    let mut second = script_step();
    second["id"] = json!("22222222-2222-4222-8222-222222222222");
    let wire = json!({"jobId":"33333333-3333-4333-8333-333333333333", "jobDisplayName":"Demo", "jobName":"demo",
        "environmentVariables":[{"type":2,"map":[{"Key":"A","Value":"outer"}]}],
        "defaults":[{"type":2,"map":[]}],"steps":[script_step(),second]});
    let job = Job::decode(wire).expect("job");
    assert_eq!(job.steps[0].context_name.as_deref(), Some("inspect_me"));
    assert_ne!(job.steps[0].id, job.steps[1].id);
    assert_eq!(job.environment.len(), 1);
    assert_eq!(job.defaults.len(), 1);
}

proptest::proptest! {
    #[test]
    fn literal_text_is_preserved(text in ".{0,256}") {
        let decoded = decode_template(json!(text.clone())).expect("literal");
        proptest::prop_assert_eq!(decoded.as_literal(), Some(text.as_str()));
    }
}
