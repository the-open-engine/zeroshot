//! Pi's memory extension, run by Node against the events Pi hands an extension.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::time::{Duration, Instant};

use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_candidate::test_support::TestDirectory;

const TOKEN: &str = "capability-token";

/// A broker that answers one tool call after another with `replies` and reports what the
/// extension acknowledged for each.
fn broker(capability: &Path, replies: Vec<String>) -> std::thread::JoinHandle<Vec<Option<u8>>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).assert_value();
    listener.set_nonblocking(true).assert_value();
    let endpoint = listener.local_addr().assert_value().to_string();
    std::fs::write(
        capability,
        json!({"endpoint":endpoint,"token":TOKEN}).to_string(),
    )
    .assert_value();
    std::thread::spawn(move || {
        let answer = |reply: String| {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "extension did not ask the broker"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("callback accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).assert_value();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .assert_value();
            let mut header = [0; 4];
            stream.read_exact(&mut header).assert_value();
            let mut body = vec![0; u32::from_be_bytes(header) as usize];
            stream.read_exact(&mut body).assert_value();
            let request: Value = serde_json::from_slice(&body).assert_value();
            assert_eq!(request["token"], TOKEN);
            assert_eq!(request["event"]["input"], json!({"path":"README.md"}));
            let mut frame = (reply.len() as u32).to_be_bytes().to_vec();
            frame.extend_from_slice(reply.as_bytes());
            stream.write_all(&frame).assert_value();
            let mut receipt = [0];
            match stream.read_exact(&mut receipt) {
                Ok(()) => Some(receipt[0]),
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset
                    ) =>
                {
                    None
                }
                Err(error) => panic!("callback receipt failed: {error}"),
            }
        };
        replies.into_iter().map(answer).collect()
    })
}

fn node() -> std::process::Command {
    let executable = crate::execution::platform::find_executable(
        "node",
        &BTreeMap::from([("PATH".to_owned(), std::env::var("PATH").assert_value())]),
        &std::env::current_dir().assert_value(),
    )
    .expect("Node.js must be installed on the test PATH");
    let mut command = std::process::Command::new(executable);
    command.env_clear();
    command
}

// Three tool calls and the turn ends around them, as Pi 1.4.0 raises them: `tool_result` with
// the call and its result, and `turn_end` with the entries the boundary holds so far
// (`BoundaryState`), answered with the entries to keep (`BoundaryResult`).
const TURN: &str = r#"import {pathToFileURL} from 'node:url';
const {default:extension}=await import(pathToFileURL(process.argv[1]).href);
const handlers={};
const sent=[];
extension({on:(name,handler)=>{handlers[name]=handler;},sendUserMessage:(text,options)=>sent.push({text,options})});
const ctx={cwd:process.argv[2]};
const call=async (toolName)=>{
  const event={toolName,toolCallId:'call_'+toolName,input:{path:'README.md'},content:[{type:'text',text:'original'}],isError:false};
  const before=JSON.stringify(event);
  const returned=await handlers.tool_result(event,ctx);
  return {returned:returned??null,unchanged:JSON.stringify(event)===before};
};
const kept={type:'custom',customType:'kept'};
const calls=[await call('read'),await call('bash')];
const first=await handlers.turn_end({entries:[kept]},ctx);
const second=await handlers.turn_end({entries:[kept]},ctx);
calls.push(await call('edit'));
const third=await handlers.turn_end({entries:[kept]},ctx);
console.log(JSON.stringify({sent,calls,first:first??null,second:second??null,third:third??null}));"#;

#[test]
fn the_extension_sends_a_steer_as_a_user_message_and_adds_lessons_once_at_the_turn_end() {
    let directory = TestDirectory::new("pi-memory-extension");
    let capability = directory.path().join("active-memory-capability.json");
    let steer = "<steer>\n[id] Standing direction\n</steer>";
    let receipts = broker(
        &capability,
        vec![
            json!({"context":"[lesson@1] first","steer":steer}).to_string(),
            json!({"context":"[lesson@2] second","steer":""}).to_string(),
            String::new(),
        ],
    );
    let mut command = ProcessSessionCommand {
        program: "pi".to_owned(),
        argv: Vec::new(),
        environment: BTreeMap::new(),
        workspace: WorkspaceCapability {
            current_dir: directory.path().to_owned(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: None,
    };
    super::super::memory::install(&mut command, &capability).assert_value();
    assert_eq!(command.argv[0], "--extension");
    let output = node()
        .args(["--input-type=module", "-e", TURN, &command.argv[1]])
        .arg(directory.path())
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let returned: Value = serde_json::from_slice(&output.stdout).assert_value();
    // The steer goes to Pi's steering queue as a user message. No tool result is changed.
    assert_eq!(
        returned["sent"],
        json!([{"text":steer,"options":{"deliverAs":"steer"}}])
    );
    let untouched = json!({"returned":null,"unchanged":true});
    assert_eq!(returned["calls"], json!([untouched, untouched, untouched]));
    // The lessons of one model response join the session together, after the entries Pi
    // already holds, and only once.
    let lesson = |content: &str| json!({"type":"custom_message","customType":"zeroshot-memory","content":content,"display":true});
    assert_eq!(
        returned["first"],
        json!({"entries":[{"type":"custom","customType":"kept"},lesson("[lesson@1] first"),lesson("[lesson@2] second")]})
    );
    assert_eq!(returned["second"], Value::Null);
    assert_eq!(returned["third"], Value::Null);
    assert_eq!(
        receipts.join().assert_value(),
        vec![Some(1), Some(1), None],
        "a hand-over is acknowledged; a tool call without one acknowledges nothing"
    );
}
