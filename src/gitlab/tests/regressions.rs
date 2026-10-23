use super::*;

fn render(input: Value) -> crate::error::Result<trace::Trace> {
    let context = context::Context::decode(&input, "admin", "runner")?;
    trace::render(
        &super::super::model::Job::decode(input)?,
        &context.redactor,
        0,
    )
}

#[test]
fn decoded_native_url_credentials_are_masked() {
    let mut input = native_job();
    input["run"] = json!(
        r#"[{"name":"reference","step":"https:\/\/u:escaped-password@example.test/ref","inputs":{"copy":"escaped-password"}}]"#
    );
    let text = String::from_utf8(render(input).expect("trace").bytes).expect("utf8");
    assert!(!text.contains("escaped-password"));
}

#[test]
fn native_numbers_reject_precision_loss_in_inputs_outputs_and_nested_values() {
    for number in ["18446744073709551617", "0.123456789012345678901", "1e-999"] {
        for values in [
            format!(r#"{{"number":{number}}}"#),
            format!(r#"{{"nested":[{{"number":{number}}}]}}"#),
        ] {
            for run in [
                format!(r#"[{{"name":"reference","step":"ref","inputs":{values}}}]"#),
                format!(r#"[{{"run":[],"outputs":{values}}}]"#),
            ] {
                let mut input = native_job();
                input["run"] = json!(run);
                let error = render(input.clone())
                    .err()
                    .expect("precision loss rejected");
                assert!(error.to_string().contains("numeric precision"), "{error}");
                assert!(super::super::model::Job::decode(input).is_err());
            }
        }
    }
}

#[test]
fn native_numbers_preserve_exact_values_with_alternate_spellings() {
    let mut input = native_job();
    input["run"] = json!(
        r#"[{"run":[{"name":"reference","step":"ref","inputs":{"integer":18446744073709551615,"decimal":1.2500,"nested":[125e-2,{"decimal":0.0125e+2}]}}],"outputs":{"integer":-9223372036854775808,"decimal":1.25,"nested":[1.25E+0,{"decimal":1250e-3}]}}]"#
    );
    let text = String::from_utf8(render(input).expect("trace").bytes).expect("utf8");
    assert_eq!(text.matches("1.25").count(), 6, "{text}");
    for number in ["18446744073709551615", "-9223372036854775808"] {
        assert!(text.contains(number), "{text}");
        assert!(
            !text.contains(&format!("\"{number}\"")),
            "number became string"
        );
    }
}

#[test]
fn native_reserved_key_objects_keep_their_types_and_fields() {
    let key = "$serde_json::private::Number";
    for object in [
        format!(r#"{{"{key}":"123"}}"#),
        format!(r#"{{"{key}":"123","other":true}}"#),
        format!(r#"{{"other":true,"{key}":"123"}}"#),
    ] {
        let mut input = native_job();
        input["run"] = json!(format!(
            r#"[{{"run":[{{"name":"reference","step":"ref","inputs":{{"value":{object},"nested":[{object}]}}}}],"outputs":{{"value":{object},"nested":[{object}]}}}}]"#
        ));
        let job = super::super::model::Job::decode(input.clone()).expect("native objects");
        let value = serde_json::to_value(&job.native_steps).expect("native JSON");
        for pointer in [
            "/0/run/0/inputs/value",
            "/0/run/0/inputs/nested/0",
            "/0/outputs/value",
            "/0/outputs/nested/0",
        ] {
            let object_value = value.pointer(pointer).expect("nested object");
            assert!(object_value.is_object(), "{pointer}: {object_value}");
            assert_eq!(object_value[key], "123");
            if object.contains("other") {
                assert_eq!(object_value["other"], true);
            }
        }
        let text = String::from_utf8(render(input).expect("trace").bytes).expect("utf8");
        assert_eq!(text.matches(key).count(), 4, "{text}");
    }
}

#[test]
fn native_numeric_audit_skips_escaped_strings_and_string_keys() {
    let mut input = native_job();
    let values = json!({
        "18446744073709551617": "18446744073709551617",
        "quoted\"key\\": "{\"number\":0.123456789012345678901} and \\ 1e999",
        "unicode": "é\n\t\"123\"",
        "number": 1.25,
    });
    input["run"] =
        json!(json!([{"name":"reference","step":"ref","inputs":{"data":values}}]).to_string());
    let job = super::super::model::Job::decode(input.clone()).expect("strings are not numbers");
    assert_eq!(
        serde_json::to_value(&job.native_steps).expect("JSON")[0]["inputs"]["data"],
        values
    );
    render(input).expect("trace");
}

#[test]
fn native_key_value_objects_are_public_declarations() {
    let mut input = native_job();
    input["run"] = json!(
        r#"[{"run":[{"name":"reference","step":"ref","inputs":{"key":"GREETING","value":"renderable-greeting-478"}}],"outputs":{"nested":[{"key":"RESULT","value":"renderable-output-479","masked":true,"file":true}]}}]"#
    );
    let text = String::from_utf8(render(input).expect("trace").bytes).expect("utf8");
    for public in ["renderable-greeting-478", "renderable-output-479"] {
        assert!(text.contains(public), "{text}");
    }
}

#[test]
fn only_actual_variable_collections_supply_sensitivity_flags() {
    for location in ["/variables", "/image/variables", "/services/0/variables"] {
        for flag in [
            json!({}),
            json!({"public":true,"masked":true}),
            json!({"public":true,"file":true}),
        ] {
            let mut input = native_job();
            input["image"]["variables"] = json!([]);
            let mut variable = flag;
            variable["key"] = json!("PRIVATE");
            variable["value"] = json!("private-collection-value-480");
            *input.pointer_mut(location).expect("variable collection") = json!([variable]);
            input["run"] = json!(
                r#"[{"name":"reference","step":"ref","inputs":{"copy":"private-collection-value-480","nested":{"https:\/\/u:escaped-password-481@example.test":"escaped-password-481"}}}]"#
            );
            let text = String::from_utf8(render(input).expect("trace").bytes).expect("utf8");
            assert!(!text.contains("private-collection-value-480"), "{location}");
            assert!(!text.contains("escaped-password-481"), "{location}");
        }
    }
}

#[test]
fn preparation_accounts_for_rendered_number_bytes() {
    for spelling in [
        "18446744073709551615",
        "-9223372036854775808",
        "1.25",
        "1e100",
    ] {
        let value: Value = serde_json::from_str(spelling).expect("JSON number");
        assert_eq!(
            super::super::preparation::validate(&value).expect("budget"),
            value.to_string().len() + 32
        );
    }
}

#[test]
fn sensitive_scalar_spellings_are_masked_without_changing_other_types() {
    let mut input = native_job();
    input["variables"] = json!([
        {"key":"NUMBER", "value":"123456", "masked":true},
        {"key":"BOOL", "value":"true", "masked":true},
        {"key":"NULL", "value":"null", "masked":true}
    ]);
    input["run"] = json!(
        r#"[{"name":"reference","step":"ref","inputs":{"number":123456,"boolean":true,"empty":null,"other":42,"off":false}}]"#
    );
    let text = String::from_utf8(render(input).expect("trace").bytes).expect("utf8");
    for secret in ["123456", "true", "null"] {
        assert!(!text.contains(secret), "{secret}");
    }
    assert!(text.contains("\"other\": 42"));
    assert!(text.contains("\"off\": false"));
}

#[test]
fn nullable_variables_preserve_null_in_job_and_service_declarations() {
    let mut input = job();
    input["variables"] = json!([{"key":"EMPTY", "value":null, "public":true}]);
    input["services"][0]["variables"] = input["variables"].clone();
    let text = String::from_utf8(render(input).expect("trace").bytes).expect("utf8");
    assert_eq!(text.matches("\"value\": null").count(), 2);
}

#[test]
fn colliding_masked_keys_fail_without_silently_dropping_declarations() {
    let mut input = native_job();
    input["variables"] = json!([{"key":"SECRET", "value":"secret", "masked":true}]);
    input["run"] =
        json!(r#"[{"name":"reference","step":"ref","inputs":{"mapping":{"secret":1,"***":2}}}]"#);
    assert!(render(input).is_err());
}

#[test]
fn preparation_bounds_work_without_rejecting_large_low_cost_scripts() {
    let mut input = job();
    input["variables"] = json!([]);
    input["credentials"] = json!([]);
    input["dependencies"] = json!([]);
    input["services"] = json!([]);
    input["git_info"]["repo_url"] = json!("https://example.test/repo");
    input["steps"][0]["script"] = json!(["x".repeat(1024 * 1024)]);
    assert!(
        render(input.clone()).is_ok(),
        "one MiB script with modest masking cost"
    );
    input["variables"] = json!(
        (0..90)
            .map(|n| json!({"key":format!("KEY{n}"), "value":format!("secret-{n}"), "masked":true}))
            .collect::<Vec<_>>()
    );
    assert!(context::Context::decode(&input, "admin", "runner").is_err());
}

#[test]
fn preparation_rejects_marker_expansion_and_excessive_metadata() {
    for secret in ["*", "prefix\n**", &"x".repeat(33 * 1024)] {
        let mut input = job();
        input["variables"] = json!([{"key":"SECRET", "value":secret, "masked":true}]);
        assert!(render(input).is_err());
    }
}
