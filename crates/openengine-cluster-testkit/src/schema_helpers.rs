use serde_json::Value;

pub(crate) fn merge_schema(root: &mut Value, name: &str, mut component: Value) {
    if let Some(definitions) = component.get_mut("$defs").and_then(Value::as_object_mut) {
        let definitions = std::mem::take(definitions);
        root["$defs"]
            .as_object_mut()
            .expect("root schema has definitions")
            .extend(definitions);
    }
    component
        .as_object_mut()
        .expect("schema root is an object")
        .remove("$schema");
    component
        .as_object_mut()
        .expect("schema root is an object")
        .remove("$defs");
    root["$defs"][name] = component;
}
