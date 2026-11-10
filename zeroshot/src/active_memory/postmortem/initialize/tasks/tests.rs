use super::*;

#[test]
fn exported_task_digest_binds_the_complete_original_definition_file() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("session");
    files::create_directory(&directory).unwrap();
    let definition_path = directory.join("original-definition.json");
    let original = json!({"authored_graph":{"node":"late-reviewer"},"authored_runtime":{"nodes":{"late-reviewer":{"model":"original"}}}});
    files::write(&definition_path, &original).unwrap();
    let mut context = json!({"evidence":{"authored_definition_file":definition_path}});
    context["evidence"]["authored_definition_digest"] =
        json!(definition_digest(&context).unwrap().unwrap());
    let task = Task {
        instructions: "Inspect the complete original definition".to_owned(),
        workspace: directory.clone(),
        context,
        output_contract: json!({}),
        response_format: json!({}),
    };
    let digest = files::digest(&task).unwrap();
    let reference = TaskRef {
        path: "task.json".to_owned(),
        digest: digest.clone(),
        role: Role::GraphMaintenance,
    };
    files::write(&directory.join(&reference.path), &task).unwrap();
    files::write(
        &directory.join("task.json.response.json"),
        &json!({"task_digest":digest,"output":{"decisions":[]}}),
    )
    .unwrap();
    assert!(response(&directory, &reference).is_ok());
    files::write(
        &definition_path,
        &json!({"authored_graph":"changed","authored_runtime":{}}),
    )
    .unwrap();
    assert!(
        response(&directory, &reference)
            .unwrap_err()
            .to_string()
            .contains("original authored definition changed")
    );
}
