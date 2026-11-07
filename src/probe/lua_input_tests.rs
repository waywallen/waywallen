use super::{repo, seed_typed, service, task};
use std::io::Write;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

struct Worker {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    id: u32,
}

impl Worker {
    async fn open(root: &Path, script: &str) -> Option<Self> {
        let tool = std::env::var_os("WAYWALLEN_TEST_PROBE")?;
        std::fs::write(root.join("probe.lua"), script).unwrap();
        let mut child = Command::new(tool)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut worker = Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            id: 0,
        };
        assert_eq!(worker.event().await["event"], "ready");
        worker
            .send(serde_json::json!({"event":"configure","programs":[{
                "key":"inputs", "root":root, "entry":"probe.lua", "files":["probe.lua"],
                "version":1,"types":["scene"]
            }]}))
            .await;
        assert_eq!(worker.event().await["event"], "configured");
        Some(worker)
    }

    async fn send(&mut self, value: serde_json::Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.unwrap();
    }

    async fn event(&mut self) -> serde_json::Value {
        let line = tokio::time::timeout(Duration::from_secs(15), self.output.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("probe must return a response");
        serde_json::from_str(&line).unwrap()
    }

    async fn start(&mut self, path: &Path) {
        self.id += 1;
        self.send(serde_json::json!({
            "request_id":self.id.to_string(),"path":path,"kind":"lua:inputs:scene:",
            "source":"test","wp_type":"scene","sub_type":"","selection_version":1
        }))
        .await;
    }

    async fn result(&mut self) -> serde_json::Value {
        loop {
            let event = self.event().await;
            assert_eq!(event["request_id"], self.id.to_string());
            if event["event"] == "result" {
                return event;
            }
            assert_eq!(event["event"], "resolved");
        }
    }

    async fn close(mut self) {
        drop(self.input);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}

#[tokio::test]
async fn json_conversion_uses_the_parser_depth_budget() {
    let root = tempfile::tempdir().unwrap();
    let Some(probe) = service(root.path(), "local fs=require('waywallen.fs'); local json=require('waywallen.json'); return {probe=function(item) local value=json.parse(fs.read(item.path)); while type(value)=='table' do value=value.value end; return {width=value,height=1} end}", &[]) else {return;};
    let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
    for (depth, expected) in [(33, "ok"), (127, "ok"), (128, "failed"), (160, "failed")] {
        let mut value = serde_json::json!(1920);
        for _ in 0..depth {
            value = serde_json::json!({"value":value});
        }
        let name = format!("depth-{depth}.json");
        std::fs::write(root.path().join(&name), serde_json::to_vec(&value).unwrap()).unwrap();
        let id = seed_typed(&db, root.path(), &name, "", "scene").await;
        task::run_pending(&db, probe.clone()).await.unwrap();
        let state = repo::probe_state(&db, id).await.unwrap().unwrap();
        assert_eq!(
            state.status.as_deref(),
            Some(expected),
            "depth={depth}: {state:?}"
        );
        if expected == "ok" {
            assert_eq!((state.width, state.height), (Some(1920), Some(1)));
        }
    }
    assert_eq!(task::run_pending(&db, probe).await.unwrap().media_probed, 0);
}

#[tokio::test]
async fn buffered_ranges_cross_boundaries_seek_and_keep_input_limits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("input");
    let bytes: Vec<u8> = (0..256 * 1024).map(|i| 32 + (i % 95) as u8).collect();
    std::fs::write(&path, &bytes).unwrap();
    std::fs::write(root.path().join("other"), b"other input").unwrap();
    let script = r#"
local fs=require('waywallen.fs')
return {probe=function(item)
    local path=item.path
    local size=fs.stat(path).size
    local function byte(offset) return 32+offset%95 end
    local function check(offset,length)
        local text=fs.read_at(path,offset,length)
        assert(#text==length)
        for i=1,length do assert(text:byte(i)==byte(offset+i-1)) end
    end
    for offset=0,20000,4 do
        assert(fs.u32_le(path,offset)==byte(offset)+byte(offset+1)*256+byte(offset+2)*65536+byte(offset+3)*16777216)
    end
    check(8190,17)
    check(200000,32769)
    check(3,101)
    assert(fs.read_at(fs.join(fs.parent(path),'other'),0,5)=='other')
    check(104,64)
    check(size-95,95)
    assert(fs.read_at(path,size,0)=='')
    assert(not pcall(fs.read_at,path,size+1,0))
    assert(not pcall(fs.read_at,path,-1,1))
    assert(not pcall(fs.read_at,path,0,-1))
    assert(not pcall(fs.read_at,path,size-1,2))
    assert(not pcall(fs.u32_le,path,size-3))
    assert(not pcall(fs.read_at,path,0,8*1024*1024+1))
    check(0,64)
    return {width=1,height=1}
end}
"#;
    let Some(mut worker) = Worker::open(root.path(), script).await else {
        return;
    };
    worker.start(&path).await;
    let result = worker.result().await;
    assert_eq!(result["status"], "ok", "{result}");
    assert!(result["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["path"] == root.path().join("other").to_str().unwrap()));
    worker.close().await;
}

#[tokio::test]
async fn buffered_hits_still_consume_the_logical_read_budget() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("input");
    std::fs::write(&path, vec![b'x'; 17 * 1024 * 1024]).unwrap();
    let script = "local fs=require('waywallen.fs'); return {probe=function(item) for i=0,4095 do assert(#fs.read_at(item.path,i*4096,4096)==4096) end; return {width=1,height=1} end}";
    let Some(mut worker) = Worker::open(root.path(), script).await else {
        return;
    };
    worker.start(&path).await;
    let result = worker.result().await;
    // Loading probe.lua also consumes this request's logical budget.
    assert_eq!(result["status"], "failed", "{result}");
    assert!(result["error"]
        .as_str()
        .unwrap()
        .contains("read budget exceeded"));
    worker.close().await;
}

#[tokio::test]
async fn exhausted_read_ahead_budget_falls_back_to_exact_reads() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("input");
    std::fs::write(&path, vec![b'a'; 16384]).unwrap();
    let script = "local fs=require('waywallen.fs'); return {probe=function(item) for i=1,3000 do assert(fs.u32_le(item.path,0)==1633771873) end; return {width=1,height=1} end}";
    let Some(mut worker) = Worker::open(root.path(), script).await else {
        return;
    };
    worker.start(&path).await;
    let result = worker.result().await;
    assert_eq!(result["status"], "ok", "{result}");
    worker.close().await;
}

#[tokio::test]
async fn buffered_inputs_are_reopened_between_requests() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("input");
    std::fs::write(&path, vec![b'a'; 16384]).unwrap();
    let script = "local fs=require('waywallen.fs'); local calls=0; return {probe=function(item) calls=calls+1; local data=fs.read_at(item.path,0,3); assert(data==(calls==1 and 'aaa' or 'bbb')); return {width=calls,height=1} end}";
    let Some(mut worker) = Worker::open(root.path(), script).await else {
        return;
    };
    worker.start(&path).await;
    assert_eq!(worker.result().await["status"], "ok");
    std::fs::write(root.path().join("replacement"), vec![b'b'; 16384]).unwrap();
    std::fs::rename(root.path().join("replacement"), &path).unwrap();
    worker.start(&path).await;
    let result = worker.result().await;
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(result["width"], 2);
    worker.close().await;
}

#[tokio::test]
async fn buffered_input_changes_are_discarded_and_short_reads_recover() {
    for replace in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input");
        std::fs::write(&path, vec![b'a'; 16384]).unwrap();
        let script = r#"
local fs=require('waywallen.fs')
return {probe=function(item)
    assert(fs.read_at(item.path,0,3)=='aaa')
    fs.exists(fs.join(fs.parent(item.path),'checkpoint'))
    while fs.stat(item.path).size~=1 do end
    local ok,value=pcall(fs.u32_le,item.path,0)
    assert(ok~=expect_short_read)
    if ok then assert(value==1633771873)
    else assert(fs.read_at(item.path,0,1)=='X') end
    return {width=2,height=2}
end}
"#;
        let script = format!("local expect_short_read={}\n{script}", !replace);
        let Some(mut worker) = Worker::open(root.path(), &script).await else {
            return;
        };
        worker.start(&path).await;
        loop {
            let event = worker.event().await;
            assert_eq!(event["event"], "resolved", "{event}");
            if event["dependencies"].as_array().unwrap().iter().any(|d| {
                d["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("/checkpoint"))
            }) {
                break;
            }
        }
        if replace {
            std::fs::write(root.path().join("replacement"), b"X").unwrap();
            std::fs::rename(root.path().join("replacement"), &path).unwrap();
        } else {
            let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.write_all(b"X").unwrap();
            file.set_len(1).unwrap();
        }
        let result = worker.result().await;
        assert_eq!(result["status"], "input_changed", "{result}");
        assert_eq!(result["width"], 2, "{result}");
        assert!(result["error"].is_null(), "{result}");
        worker.close().await;
    }
}
