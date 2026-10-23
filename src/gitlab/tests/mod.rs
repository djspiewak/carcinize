use super::*;
use serde_json::{Value, json};

mod lifecycle;
mod model;
mod regressions;

fn job() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/gitlab/script.json")).expect("fixture")
}

fn native_job() -> Value {
    let mut value = job();
    value.as_object_mut().expect("job").remove("steps");
    value["run"] = json!(serde_json::to_string(&json!([
        {"name":"script", "script":"exit 97", "env":{"DECLARED":"${{ job.PUBLIC }}"}},
        {"name":"reference", "step":{"git":{"url":"https://u:native-password@example.test/step.git","rev":"main","dir":"nested"}},"inputs":{"flag":true,"count":3,"nested":[null,{"key":"value"}]}},
        {"name":"action", "action":"unavailable/action@v1"},
        {"run":[{"exec":{"command":["/missing","--flag"],"work_dir":"/absent"}}],"outputs":{"result":"${{ steps.script.outputs.value }}"}},
        {"steps":[{"name":"legacy", "script":"exit 98"}],"delegate":"child"}
    ])).expect("native JSON"));
    value
}
