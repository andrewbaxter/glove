use {
    schemars::JsonSchema,
    serde::{
        Deserialize,
        Serialize,
    },
    std::io::ErrorKind,
    tokio::{
        io::{
            AsyncReadExt,
            AsyncWriteExt,
        },
        net::UnixStream,
    },
};

#[doc(hidden)]
pub mod republish {
    pub use serde;
    pub use serde_json;
    pub use tokio;
    pub use libc;
    pub use rustix;
    pub use defer;
    pub use schemars;
}

pub type Error = String;

#[doc(hidden)]
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Resp<T> {
    Ok(T),
    Err(Error),
}

#[doc(hidden)]
pub async fn write_framed(conn: &mut UnixStream, message: &[u8]) -> Result<(), Error> {
    conn.write_u64_le(message.len() as u64).await.map_err(|e| format!("Error writing frame size: {}", e))?;
    conn.write_all(message).await.map_err(|e| format!("Error writing frame body: {}", e))?;
    return Ok(());
}

#[doc(hidden)]
pub async fn read_framed(conn: &mut UnixStream) -> Result<Option<Vec<u8>>, Error> {
    let len = match conn.read_u64_le().await {
        Ok(len) => len,
        Err(e) => {
            match e.kind() {
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof => {
                    return Ok(None);
                },
                _ => { },
            }
            return Err(format!("Error reading message length from connection: {}", e));
        },
    };
    let mut body = vec![];
    body.reserve(len as usize);
    match conn.read_buf(&mut body).await {
        Ok(_) => { },
        Err(e) => {
            return Err(format!("Error reading message body from connection: {}", e));
        },
    }
    return Ok(Some(body));
}

/// See readme for details. This builds the request enum with trait with associated
/// response types, client and server structs, and methods for sending/receiving
/// messages.
#[macro_export]
macro_rules! reqresp{
    ($vis: vis $name: ident {
        $($req_name: ident($req_type: ty) => $resp_type: ty,) *
    }) => {
        $vis mod $name {
            use {
                $crate:: {
                    republish:: {
                        serde:: {
                            Serialize,
                            Deserialize,
                            de::DeserializeOwned
                        },
                        serde_json,
                        tokio:: {
                            self,
                            net:: {
                                UnixStream,
                                UnixSocket,
                                UnixListener
                            },
                            fs:: {
                                File
                            },
                        },
                        rustix:: {
                            self,
                            fd::AsFd
                        },
                        libc,
                        defer,
                        schemars::JsonSchema,
                    },
                    read_framed,
                    write_framed,
                    Error,
                    Resp,
                },
            };
            #[allow(unused_imports)]
            use {
                super::*,
            };
            //. .
            /// Generate a mapping of request and response names to the associated type json schema. 
            /// This is primarily intended for documentation generation (serializing schemas).
                pub fn to_json_schema() -> std:: collections:: HashMap < String,
                schemars::schema::RootSchema > {
                    return[
                        $(
                            (stringify!($req_name).to_string(), schemars::schema_for!($req_type)),
                            (format!("{}Resp", stringify!($req_name)), schemars::schema_for!($resp_type))
                            //. .
                            ,
                        ) *
                    ].into_iter().collect();
                }
            //. .
            pub struct ServerResp(Vec<u8>);
            impl ServerResp {
                /// Create a generic (non request-specific) error response.
                pub fn err(e: impl Into<String>) -> ServerResp {
                    return ServerResp(serde_json::to_vec(&Resp::<()>::Err(e.into())).unwrap());
                }
            }
            //. .
            pub enum ServerReq {
                $(
                    /// A request of type $req_type. This is a 2-tuple, with the first element being a
                    /// "responder" which type-checks the response data and produces a `ServerResp`.
                    /// The second element is the request data itself. See the readme for canonical
                    /// usage.
                    $req_name(fn($resp_type) -> ServerResp, $req_type),) *
            }
            //. .
            #[
                doc(hidden)
            ] #[
                derive(Serialize, Deserialize, JsonSchema)
            ] #[serde(rename_all = "snake_case", deny_unknown_fields)] pub enum Req {
                $($req_name($req_type),) *
            }
            //. .
            #[doc(hidden)]
            pub trait ReqTrait {
                type Resp: Serialize + DeserializeOwned;

                fn to_enum(self) -> Req;
            }
            //. .
            impl Req {
                fn to_server_req(self) -> ServerReq {
                    match self {
                        $(
                            Self:: $req_name(
                                req_inner
                            ) => ServerReq:: $req_name(
                                |resp| ServerResp(serde_json::to_vec(&Resp::Ok(resp)).unwrap()),
                                req_inner
                            ),
                        ) *
                    }
                }
            }
            //. .
            $(impl ReqTrait for $req_type {
                type Resp = $resp_type;
                fn to_enum(self) -> Req {
                    return Req:: $req_name(self);
                }
            }) * 
            // UDS
            //. .
            pub struct Client(UnixStream);
            impl Client {
                /// Connect to an existing server socket.
                pub async fn new(path: impl AsRef<std::path::Path>) -> Result<Client, String> {
                    let path = path.as_ref();
                    let conn =
                        UnixSocket::new_stream()
                            .map_err(|e| format!("Error initializing IPC unix socket: {}", e))?
                            .connect(path)
                            .await
                            .map_err(|e| format!("Error connecting to IPC socket at [{:?}]: {}", path, e))?;
                    return Ok(Self(conn));
                }

                /// Sends a request and returns the associated response, or an error.
                pub async fn send_req<I: ReqTrait>(&mut self, req: I) -> Result<I::Resp, String> {
                    let resp = self.send_req_enum(&req.to_enum()).await?;
                    let resp =
                        serde_json::from_slice::<Resp<I::Resp>>(
                            &resp,
                        ).map_err(
                            |e| format!(
                                "Error parsing IPC response: {}\nBody: {}",
                                e,
                                String::from_utf8_lossy(&resp)
                            ),
                        )?;
                    match resp {
                        Resp::Ok(v) => return Ok(v),
                        Resp::Err(e) => return Err(e),
                    }
                }
                /// Sends a tagged request and returns the unparsed response json bytes. You'll typically want `send_req` instead.
                pub async fn send_req_enum(&mut self, req: &Req) -> Result<Vec<u8>, String> {
                    write_framed(&mut self.0, &serde_json::to_vec(req).unwrap()).await?;
                        return Ok(read_framed(&mut self.0)
                            .await
                            .map_err(|e| format!("Error reading IPC response: {}", e))?
                            .ok_or_else(|| format!("Disconnected by remote host"))?);
                }
            }
            pub struct Server {
                pub sock: UnixListener,
                cleanup: Vec<Box<dyn std::any::Any + Send + Sync>>,
            }
            pub struct ServerConn(pub UnixStream);
            impl Server {
                /// Create the IPC socket. This uses file locks to prevent multiple servers from
                /// clobbering eachother.
                pub async fn new(path: impl AsRef<std::path::Path>) -> Result<Server, Error> {
                    let ipc_path = path.as_ref().to_path_buf();
                    let lock_path = ipc_path.with_extension("lock");
                    let filelock =
                        File::options()
                            .mode(0o660)
                            .write(true)
                            .create(true)
                            .custom_flags(libc::O_CLOEXEC)
                            .open(&lock_path)
                            .await
                            .map_err(|e| format!("Error opening IPC lock file at [{:?}]: {}", lock_path, e))?;
                    rustix::fs::flock(
                        filelock.as_fd(),
                        rustix::fs::FlockOperation::NonBlockingLockExclusive,
                    ).map_err(
                        |e| format!(
                            "Error getting exclusive lock for IPC socket at lock path [{:?}], is another instance using the same socket path? {}",
                            lock_path,
                            e
                        ),
                    )?;
                    let clean_lock = defer::defer(move || {
                        _ = std::fs::remove_file(&lock_path);
                    });
                    match tokio::fs::remove_file(&ipc_path).await {
                        Ok(_) => { },
                        Err(e) => match e.kind() {
                            std::io::ErrorKind::NotFound => { },
                            _ => {
                                return Err(format!("Error cleaning up old IPC socket at [{:?}]: {}", ipc_path, e));
                            },
                        },
                    }
                    let sock =
                        UnixListener::bind(
                            &ipc_path,
                        ).map_err(|e| format!("Error creating IPC listener at [{:?}]: {}", ipc_path, e))?;
                    return Ok(Server {
                        sock: sock,
                        cleanup: vec![Box::new(clean_lock), Box::new(defer::defer(move || {
                            _ = std::fs::remove_file(&ipc_path);
                        }))],
                    });
                }

                /// Wait for a client connection.
                pub async fn accept(&mut self) -> Result<ServerConn, Error> {
                    let (stream, _peer) = match self.sock.accept().await {
                        Ok((stream, peer)) => (stream, peer),
                        Err(e) => {
                            return Err(format!("Error accepting connection: {}", e));
                        },
                    };
                    return Ok(ServerConn(stream));
                }
            }
            impl ServerConn {
                /// Wait for the next message.  Returns `None` if the connection is closed,
                /// otherwise the request. The request is an enum where each variant is a 2-tuple
                /// of a "responder" and the specific request data.
                ///
                /// The responder is a function that only accepts the specific success response
                /// data type for that variant's request and produces a generic response value for
                /// the `send_resp` method. You can use this to ensure a schema-accurate response -
                /// see the readme for canonical use.
                pub async fn recv_req(&mut self) -> Result<Option<ServerReq>, Error> {
                    let req = match read_framed(&mut self.0).await? {
                        Some(m) => m,
                        None => {
                            return Ok(None);
                        },
                    };
                    let req =
                        serde_json::from_slice::<Req>(
                            &req,
                        ).map_err(
                            |e| format!("Error parsing IPC request: {}\nBody: {}", e, String::from_utf8_lossy(&req)),
                        )?;
                    return Ok(Some(req.to_server_req()));
                }

                /// Send the response to the client.  `resp` is generated by the type-checking
                /// "responder" in the request enum - see `recv_req`. An error can also be
                /// generated using `ServerResp::err(...)`.
                pub async fn send_resp(&mut self, resp: ServerResp) -> Result<(), Error> {
                    write_framed(&mut self.0, &resp.0).await?;
                    return Ok(());
                }
            }
        }
    };
}

#[cfg(test)]
mod test {
    #![allow(dead_code)]

    reqresp!(pub x {
        A(()) => i32,
        B(i32) =>(),
    });
}
