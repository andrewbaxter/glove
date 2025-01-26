This is some tooling around a simple IPC protocol consisting of JSON messages. The requests are a json enum (per `serde_json` serialization) and each request has an associated response type.

This generates code for thoroughly type-safe async communication.

Right now it provides an un-versioned unix domain socket implementation. In the future I'd like to optionally support message versioning and possibly remove the client/server in favor of `AsyncWrite`/`AsyncRead` plus generic UDS socket creation methods to support other transports.

`cargo add glove`

```rust
// # Message definition
//
// Note that request types must be unique for type-based marshaling.
#[derive(serde::Serialize, serde::Deserialize)]
struct ReqA(i32);

glove::reqresp!(pub ipc {
    A(ReqA) => i32,
    B(bool) =>(),
});

// # Typical server use:
let server = ipc::Server::new("/run/my_ipc.sock").await?;
loop {
    let conn = server.accept().await?;
    spawn(async move {
        while let Some(req) = conn.recv_req().await? {
            let resp;
            match req {
                ipc::ServerReq::A(responder, req) => {
                    resp = responder(req.0 * 2);
                },
                ipc::ServerReq::B(_responder, req) => {
                    resp = ipc::ServerResp::err("B isn't implemented yet");
                }
            }
            if let Err(e) = conn.send_resp(resp).await {
                eprintln!("Error sending client response, maybe disconnected midway");
                return;
            }
        }
    })
}

// # Typical client use:
let client = ipc::Client::new("/run/my_ipc.sock").await?;
let resp = client.send_req(ReqA(12)).await?;
eprintln!("Got resp {}", resp.0);
```
