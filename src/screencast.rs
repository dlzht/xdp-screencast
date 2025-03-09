use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use bitflags::bitflags;
use zbus::export::ordered_stream::OrderedStreamExt;
use zbus::zvariant::{ObjectPath, OwnedFd, OwnedObjectPath, Value};
use zbus::{proxy, zvariant, Connection, Error, Result};

#[derive(Debug, Default)]
pub struct ScreenCast<'a> {
    pub cursor_mode: CursorMode,
    pub source_type: SourceType,
    pub persist_mode: PersistMode,
    pub multiple_source: bool,

    selected_sources: Vec<SelectedSource>,
    connection: Option<Connection>,
    proxy: Option<ZBusScreencastProxy<'a>>,
    session: OwnedObjectPath,
    counter: usize,
}

impl<'a> ScreenCast<'a> {
    pub async fn screencast(&mut self) -> Result<RawFd> {
        let connection = Connection::session().await?;
        let proxy = ZBusScreencastProxy::new(&connection).await?;
        self.connection = Some(connection);
        self.proxy = Some(proxy);
        self.create_session().await?;
        self.prepare_select().await?;
        self.start_select().await?;
        self.open_remote().await
    }

    pub async fn shutdown(self) -> Result<()> {
        if let Some(connection) = self.connection {
            return connection.close().await;
        }
        Ok(())
    }

    async fn create_session(&mut self) -> Result<()> {
        let mut payload = HashMap::with_capacity(4);
        let session_token_value = Value::new(self.counter.to_string());
        payload.insert("session_handle_token", &session_token_value);
        let handle_token_value = Value::new(self.counter.to_string());
        payload.insert("handle_token", &handle_token_value);
        let request_path = self
            .proxy
            .as_ref()
            .unwrap()
            .create_session(&payload)
            .await?;
        self.counter += 1;

        let request_proxy = ZBusRequestProxy::builder(self.connection.as_ref().unwrap())
            .path(request_path)?
            .build()
            .await?;
        let mut responses = request_proxy.receive_response().await?;

        let response = responses.next().await.ok_or(Error::Failure(
            "create session: fail to receive response".to_string(),
        ))?;
        let mut response = response.args()?;

        let session_handle = match response.response {
            0 => response.results.remove("session_handle"),
            _ => None,
        };

        let session = session_handle
            .ok_or(Error::Failure(
                "create session: fail to get session_handle".to_string(),
            ))
            .and_then(|v| String::try_from(v).map_err(Error::Variant))
            .and_then(|v| OwnedObjectPath::try_from(v).map_err(Error::Variant))?;
        self.session = session;
        Ok(())
    }

    async fn prepare_select(&mut self) -> Result<()> {
        let mut payload = HashMap::with_capacity(8);
        let handle_token_value = Value::new(self.counter.to_string());
        payload.insert("handle_token", &handle_token_value);
        let multiple_value = Value::Bool(self.multiple_source);
        payload.insert("multiple", &multiple_value);
        let types_value = Value::U32(self.source_type.bits());
        payload.insert("types", &types_value);
        let persist_value = Value::U32(self.persist_mode.to_u32());
        payload.insert("persist_mode", &persist_value);
        let cursor_value = Value::U32(self.cursor_mode.to_u32());
        payload.insert("cursor_mode", &cursor_value);
        let request_path = self
            .proxy
            .as_ref()
            .unwrap()
            .select_sources(&self.session, &payload)
            .await?;
        self.counter += 1;

        // let request_proxy = ZBusRequestProxy::builder(self.connection.as_ref().unwrap())
        //     .path(request_path)?
        //     .build()
        //     .await?;
        // let mut responses = request_proxy.receive_response().await?;
        // let response = responses.next().await
        //   .ok_or(Error::Failure("select source: fail to receive response".to_string()))?;
        // let response = response.args()?;
        // if response.response != 0 {
        //   return Err(Error::Failure("select source: fail to get select source".to_string()));
        // }
        Ok(())
    }

    async fn start_select(&mut self) -> Result<()> {
        let mut payload = HashMap::with_capacity(4);
        let handle_token_value = Value::new(self.counter.to_string());
        payload.insert("handle_token", &handle_token_value);
        let request_path = self
            .proxy
            .as_ref()
            .unwrap()
            .start(&self.session, "", &payload)
            .await?;
        self.counter += 1;

        let request_proxy = ZBusRequestProxy::builder(self.connection.as_ref().unwrap())
            .path(request_path)?
            .build()
            .await?;
        let mut responses = request_proxy.receive_response().await?;

        let response = responses.next().await.ok_or(Error::Failure(
            "start select: fail to receive response".to_string(),
        ))?;
        let mut response = response.args()?;
        let sources = match response.response {
            0 => response.results.remove("streams"),
            _ => None,
        };
        self.selected_sources = sources
            .ok_or(Error::Failure(
                "start select: fail to get streams".to_string(),
            ))
            .and_then(|v| Vec::try_from(v).map_err(Error::Variant))?;
        Ok(())
    }


    async fn open_remote(&mut self) -> Result<RawFd> {
        self.counter += 1;
        let payload = HashMap::new();
        self
          .proxy
          .as_ref()
          .unwrap()
          .open_pipe_wire_remote(&self.session, &payload)
          .await
          .map(|fd| fd.as_raw_fd())
    }
}

#[derive(Debug)]
struct SelectedSource {
    id: u32,
    width: Option<i32>,
    height: Option<i32>,
    type_: u32,
}

impl SelectedSource {
    fn new(id: u32, type_: u32) -> Self {
        SelectedSource {
            id,
            width: None,
            height: None,
            type_,
        }
    }
}

impl TryFrom<Value<'_>> for SelectedSource {
    type Error = zvariant::Error;

    fn try_from(value: Value<'_>) -> std::result::Result<Self, Self::Error> {
        if let Value::Structure(source) = value {
            if let [Value::U32(id), Value::Dict(source)] = source.fields() {
                let source_type_key = Value::new("source_type");
                let source_type: u32 =
                    source
                        .get(&source_type_key)?
                        .ok_or(zvariant::Error::Message(
                            "fail to get source_type".to_string(),
                        ))?;
                let mut result = SelectedSource::new(*id, source_type);
                let source_size_key = Value::new("size");
                if let Some(Value::Structure(size)) = source.get(&source_size_key)? {
                    if let (Some(Value::I32(width)), Some(Value::I32(height))) =
                        (size.fields().first(), size.fields().get(1))
                    {
                        result.width = Some(*width);
                        result.height = Some(*height);
                    }
                }
                return Ok(result);
            }
        }
        Err(zvariant::Error::IncorrectType)
    }
}

#[proxy(
    interface = "org.freedesktop.portal.ScreenCast",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop"
)]
pub trait ZBusScreencast {
    /// CreateSession method
    fn create_session(&self, options: &HashMap<&str, &Value<'_>>) -> Result<OwnedObjectPath>;

    /// SelectSources method
    fn select_sources(
        &self,
        session_handle: &ObjectPath<'_>,
        options: &HashMap<&str, &Value<'_>>,
    ) -> Result<OwnedObjectPath>;

    /// Start method
    fn start(
        &self,
        session_handle: &ObjectPath<'_>,
        parent_window: &str,
        options: &HashMap<&str, &Value<'_>>,
    ) -> Result<OwnedObjectPath>;

    /// OpenPipeWireRemote method
    fn open_pipe_wire_remote(
        &self,
        session_handle: &ObjectPath<'_>,
        options: &HashMap<&str, &Value<'_>>,
    ) -> Result<OwnedFd>;

    /// AvailableCursorModes property
    #[zbus(property)]
    fn available_cursor_modes(&self) -> Result<u32>;

    /// AvailableSourceTypes property
    #[zbus(property)]
    fn available_source_types(&self) -> Result<u32>;

    /// version property
    #[zbus(property, name = "version")]
    fn version(&self) -> Result<u32>;
}

#[proxy(
    interface = "org.freedesktop.portal.Request",
    default_service = "org.freedesktop.portal.Desktop",
    // default_path = "/org/freedesktop/portal/desktop"
)]
pub trait ZBusRequest {
    /// Close method
    fn close(&self) -> Result<()>;

    /// Response signal
    #[zbus(signal)]
    fn response(&self, response: u32, results: HashMap<&str, Value<'_>>) -> Result<()>;
}

#[derive(Debug, Copy, Clone)]
pub enum PersistMode {
    DoNotPersist,
    AsApplication,
    UntilRevoked,
}

impl Default for PersistMode {
    fn default() -> Self {
        PersistMode::DoNotPersist
    }
}

impl PersistMode {
    pub fn to_u32(&self) -> u32 {
        match self {
            PersistMode::DoNotPersist => 0,
            PersistMode::AsApplication => 1,
            PersistMode::UntilRevoked => 2
        }
    }
}

bitflags! {
  #[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
  pub struct SourceType: u32 {
    const MONITOR = 1;
    const WINDOW = 2;
    const VIRTUAL = 4;
  }
}

#[derive(Debug, Copy, Clone)]
pub enum CursorMode {
    Hidden,
    Embedded,
    Metadata,
}

impl Default for CursorMode {
    fn default() -> Self {
        CursorMode::Hidden
    }
}

impl CursorMode {
    pub fn to_u32(&self) -> u32 {
        match self {
            CursorMode::Hidden => 1,
            CursorMode::Embedded => 2,
            CursorMode::Metadata => 4,
        }
    }
}
