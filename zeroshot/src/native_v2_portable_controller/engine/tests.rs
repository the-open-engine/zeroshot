use super::*;

#[tokio::test]
async fn removal_requires_explicit_completion_even_when_the_sender_disappears() {
    for completed in [false, true] {
        let (sender, receiver) = watch::channel(false);
        if completed {
            sender.send_replace(true);
        }
        drop(sender);
        assert_eq!(wait_for_removability(receiver).await, completed);
    }
}

#[tokio::test]
async fn cancelled_supervisor_task_is_an_explicit_failure() {
    let task = tokio::spawn(std::future::pending::<
        Result<(), crate::native_v2_supervisor::NativeV2SupervisorError>,
    >());
    task.abort();
    assert_eq!(
        task_result(task.await),
        Err("supervisor task was cancelled".to_owned())
    );
}
