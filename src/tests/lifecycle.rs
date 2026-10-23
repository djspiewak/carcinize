use super::github_fake::{Mode, exercise};

#[tokio::test]
async fn handles_two_jobs_with_live_and_archived_logs_then_removes_its_runner() {
    let (fake, result) = exercise(Mode::Success).await;
    assert_eq!(result.expect("successful runner").jobs_inspected, 2);
    let events = fake.events.lock().expect("events");
    assert_eq!(
        events
            .iter()
            .filter(|(name, _, _)| name == "POST /actions/runners/register")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|(name, _, _)| name == "POST /run/completejob")
            .count(),
        2
    );
    let feed = events
        .iter()
        .position(|(name, _, _)| name == "feed-lines")
        .expect("live lines");
    let completion = events
        .iter()
        .position(|(name, _, _)| name == "POST /run/completejob")
        .expect("completion");
    assert!(feed < completion);
    for (name, auth, body) in events.iter() {
        if name.starts_with("PUT /blob/") {
            assert_eq!(
                auth, &None,
                "signed URLs must not receive bearer credentials"
            );
            let text = body.as_str().expect("log body");
            assert!(text.contains("exit 97"));
            assert!(!text.contains("job-secret-value"));
            assert!(!text.contains("masked-by-hint-123"));
            assert!(text.contains("***"));
        }
        if name == "feed-connect"
            || name.starts_with("POST /results/")
            || name.starts_with("POST /run/")
        {
            assert_eq!(auth.as_deref(), Some("Bearer job-token"));
        }
        if name == "POST /run/completejob" {
            assert_eq!(body["conclusion"], "succeeded");
            assert_eq!(body["stepResults"][0]["status"], "completed");
            assert_eq!(body["stepResults"][0]["conclusion"], "succeeded");
        }
    }
    assert!(events.iter().any(|(n, _, _)| n == "DELETE /broker/session"));
    assert_eq!(
        events.last().expect("cleanup").0,
        "DELETE /repos/example/project/actions/runners/42"
    );
}

#[tokio::test]
async fn failed_log_upload_cannot_report_success() {
    let (fake, _) = exercise(Mode::BrokenBlob).await;
    let events = fake.events.lock().expect("events");
    let body = &events
        .iter()
        .find(|(n, _, _)| n == "POST /run/completejob")
        .expect("failed completion")
        .2;
    assert_eq!(body["conclusion"], "failed");
    assert_eq!(body["annotations"][0]["level"], "failure");
}

#[tokio::test]
async fn unknown_step_semantics_fail_the_acquired_job() {
    let (fake, _) = exercise(Mode::BadStep).await;
    let events = fake.events.lock().expect("events");
    let body = &events
        .iter()
        .find(|(n, _, _)| n == "POST /run/completejob")
        .expect("failed completion")
        .2;
    assert_eq!(body["conclusion"], "failed");
}

#[tokio::test]
async fn session_failure_still_removes_the_new_registration() {
    let (fake, result) = exercise(Mode::BadSession).await;
    assert!(result.is_err());
    assert_eq!(
        fake.events
            .lock()
            .expect("events")
            .last()
            .expect("cleanup")
            .0,
        "DELETE /repos/example/project/actions/runners/42"
    );
}

#[tokio::test]
async fn cancellation_interrupts_reporting_and_completes_as_cancelled() {
    let (fake, _) = exercise(Mode::Cancel).await;
    let events = fake.events.lock().expect("events");
    let body = &events
        .iter()
        .find(|(n, _, _)| n == "POST /run/completejob")
        .expect("cancelled completion")
        .2;
    assert_eq!(body["conclusion"], "canceled");
}

#[tokio::test]
async fn renews_periodically_while_reporting_is_still_in_progress() {
    let (fake, result) = exercise(Mode::Renewal).await;
    assert_eq!(result.expect("runner").jobs_inspected, 1);
    assert!(fake.renewals.load(std::sync::atomic::Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn losing_the_lease_interrupts_reporting_and_prevents_success() {
    let (fake, result) = exercise(Mode::LostLease).await;
    assert_eq!(result.expect("runner").jobs_failed, 1);
    let events = fake.events.lock().expect("events");
    let body = &events
        .iter()
        .find(|(name, _, _)| name == "POST /run/completejob")
        .expect("failed completion")
        .2;
    assert_eq!(body["conclusion"], "failed");
}

#[tokio::test]
async fn invalid_feed_url_fails_the_job_and_still_cleans_up() {
    let (fake, result) = exercise(Mode::BadFeed).await;
    assert_eq!(result.expect("runner").jobs_failed, 1);
    let events = fake.events.lock().expect("events");
    let complete = events
        .iter()
        .find(|(name, _, _)| name == "POST /run/completejob")
        .expect("completion");
    assert_eq!(complete.2["conclusion"], "failed");
    assert!(!events.iter().any(|(name, _, _)| name == "feed-connect"));
    assert_eq!(
        events.last().expect("cleanup").0,
        "DELETE /repos/example/project/actions/runners/42"
    );
}

#[tokio::test]
async fn regex_masks_escaped_secrets_in_live_and_archived_output() {
    let (fake, result) = exercise(Mode::RegexEscapes).await;
    assert_eq!(result.expect("inspection").jobs_inspected, 1);
    let events = fake.events.lock().expect("events");
    let mut live = false;
    let mut archived = 0;
    for (name, _, body) in events.iter() {
        let text = if name == "feed-lines" {
            live = true;
            body.to_string()
        } else if name.starts_with("PUT /blob/") {
            archived += 1;
            body.as_str().expect("log").to_owned()
        } else {
            continue;
        };
        super::contracts::output(&text, &["secret", "quote"], &["exit 97"]);
    }
    assert!(live);
    assert_eq!(archived, 2);
}

#[tokio::test]
async fn malformed_registration_keeps_the_created_identity_for_cleanup() {
    let (fake, result) = exercise(Mode::BadRegistration).await;
    assert!(result.is_err());
    let events = fake.events.lock().expect("events");
    assert_eq!(
        events.last().expect("cleanup").0,
        "DELETE /repos/example/project/actions/runners/42"
    );
    assert!(!events.iter().any(|(name, _, _)| name == "POST /oauth"));
}

#[tokio::test]
async fn invalid_metadata_and_excessive_work_fail_the_owned_job() {
    for mode in [
        Mode::BadMetadata,
        Mode::ExcessiveMasks,
        Mode::ExpensiveRegex,
    ] {
        let (fake, result) = exercise(mode).await;
        assert_eq!(
            result.expect("failed completion acknowledged").jobs_failed,
            1
        );
        let events = fake.events.lock().expect("events");
        assert!(events.iter().any(
            |(name, _, body)| name == "POST /run/completejob" && body["conclusion"] == "failed"
        ));
        assert!(!events.iter().any(|(name, _, _)| name == "feed-connect"));
        assert_eq!(
            events.last().expect("cleanup").0,
            "DELETE /repos/example/project/actions/runners/42"
        );
    }
}

#[tokio::test]
async fn signal_errors_during_inspection_and_final_publication_survive_cleanup() {
    for mode in [Mode::SignalError, Mode::FinalSignalError] {
        let (fake, result) = exercise(mode).await;
        super::contracts::shutdown_error(result);
        let events = fake.events.lock().expect("events");
        assert!(
            events
                .iter()
                .any(|(name, _, body)| name == "POST /run/completejob"
                    && body["conclusion"] == "canceled")
        );
        assert_eq!(
            events.last().expect("cleanup").0,
            "DELETE /repos/example/project/actions/runners/42"
        );
    }
}

#[tokio::test]
async fn final_publication_is_supervised_and_drained_before_completion() {
    for (mode, expected) in [
        (Mode::FinalRenewal, "succeeded"),
        (Mode::FinalLeaseLoss, "failed"),
        (Mode::FinalShutdown, "canceled"),
    ] {
        let (fake, result) = exercise(mode).await;
        result.expect("completion acknowledged");
        assert!(
            fake.final_renewals
                .load(std::sync::atomic::Ordering::SeqCst)
                > 0
        );
        let events = fake.events.lock().expect("events");
        let published = events
            .iter()
            .position(|(name, _, _)| name.ends_with("CreateJobLogsMetadata"))
            .expect("final archive published");
        let complete = events
            .iter()
            .position(|(name, _, body)| {
                name == "POST /run/completejob" && body["conclusion"] == expected
            })
            .expect("correct terminal outcome");
        assert!(published < complete);
        assert_eq!(
            events.last().expect("cleanup").0,
            "DELETE /repos/example/project/actions/runners/42"
        );
    }
}
