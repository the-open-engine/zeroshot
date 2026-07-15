use openengine_cluster_protocol::ApplyParams;
use schemars::schema_for;
use serde_json::{json, Value};

pub(super) fn document() -> Value {
    json!({
        "openrpc": "1.3.2",
        "info": {
            "title": "Open Engine Cluster Protocol",
            "version": "1.0.0"
        },
        "methods": [
            initialize_method(),
            plan_method(),
            apply_method(),
            get_method(),
        ],
        "components": {
            "schemas": {
                "GraphSpec": { "$ref": "graph.schema.json" },
                "CompiledGraphIr": { "$ref": "compiled-ir.schema.json" },
                "GraphDiagnostic": { "$ref": "graph.schema.json#/$defs/GraphDiagnostic" },
                "StructuralBounds": { "$ref": "graph.schema.json#/$defs/StructuralBounds" },
                "ArtifactRef": { "$ref": "graph.schema.json#/$defs/ArtifactRef" }
            }
        }
    })
}

fn initialize_method() -> Value {
    json!({
        "name": "initialize",
        "paramStructure": "by-name",
        "params": [{
            "name": "protocolVersion",
            "required": true,
            "schema": {
                "type": "string",
                "const": "openengine.cluster/v1"
            }
        }],
        "result": {
            "name": "initializeResult",
            "schema": { "$ref": "schema.json#/$defs/InitializeResult" }
        }
    })
}

fn plan_method() -> Value {
    json!({
        "name": "plan",
        "paramStructure": "by-name",
        "params": [{
            "name": "graph",
            "required": true,
            "schema": { "$ref": "schema.json#/$defs/GraphSpec" }
        }],
        "result": {
            "name": "planResult",
            "schema": { "$ref": "schema.json#/$defs/PlanResult" }
        }
    })
}

fn apply_method() -> Value {
    let apply_schema = serde_json::to_value(schema_for!(ApplyParams))
        .expect("apply parameter JSON Schema serialization must succeed");
    json!({
        "name": "apply",
        "paramStructure": "by-name",
        "params": [
            {
                "name": "graph", "required": true,
                "schema": { "$ref": "schema.json#/$defs/GraphSpec" }
            },
            { "name": "input", "required": false, "schema": true },
            {
                "name": "dryRun", "required": false,
                "schema": apply_property_schema(&apply_schema, "dryRun")
            },
            {
                "name": "ifGeneration", "required": false,
                "schema": apply_property_schema(&apply_schema, "ifGeneration")
            },
            {
                "name": "idempotencyKey", "required": false,
                "schema": apply_property_schema(&apply_schema, "idempotencyKey")
            }
        ],
        "result": {
            "name": "applyResult",
            "schema": { "$ref": "schema.json#/$defs/ApplyResult" }
        }
    })
}

fn apply_property_schema(apply_schema: &Value, property: &str) -> Value {
    apply_schema["properties"]
        .get(property)
        .unwrap_or_else(|| panic!("ApplyParams schema is missing {property}"))
        .clone()
}

fn get_method() -> Value {
    json!({
        "name": "get",
        "paramStructure": "by-name",
        "params": [{
            "name": "atCursor",
            "required": false,
            "schema": { "type": ["string", "null"] }
        }],
        "result": {
            "name": "getResult",
            "schema": { "$ref": "schema.json#/$defs/GetResult" }
        }
    })
}
