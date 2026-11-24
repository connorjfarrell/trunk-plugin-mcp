use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The plugin's settings.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Port
    ///
    /// Where the MCP server listens. Add it to Claude as http://this-machine:port/mcp.
    #[schemars(range(min = 1, max = 65535))]
    pub port: u16,
    /// Listen on
    ///
    /// 127.0.0.1 is this machine only. 0.0.0.0 is every network this machine is on: then an access token is required.
    pub bind: String,
    /// Access token
    ///
    /// A client sends it as `Authorization: Bearer <token>`. Required to listen beyond this machine; it is worth having anyway,
    /// because what this serves is what was said on the radio.
    pub token: String,
    /// Networks allowed to connect
    ///
    /// Who may connect, as networks separated by commas: `172.16.0.0/12, 192.168.0.0/16` is the private networks. A client from anywhere else is refused,
    /// whatever its token. 127.0.0.0/8 is this machine.
    pub allowed_networks: String,
    /// Include encrypted calls
    ///
    /// Calls that were encrypted and that the recorder decrypted with your keys are given to the assistant, and so leave this machine
    /// for the assistant's model. Off, they are left out. A system can say otherwise in its own settings.
    pub include_encrypted: bool,
    /// Days to keep in the index
    #[schemars(range(min = 1, max = 365))]
    pub days: u32,
    /// Largest recording to send in a reply (MB)
    ///
    /// `get_audio` hands the assistant a recording only up to this size; a longer one can still be downloaded from `/audio/<call id>` with the token.
    #[schemars(range(min = 1, max = 100))]
    pub max_audio_mb: u32,
    /// Radio names
    ///
    /// A file of `radio number,name` lines (the recorder's unit names file), to give radios their names in what the assistant reads.
    pub unit_names_file: String,
    /// Where the recordings are played
    ///
    /// The address of the Playback interface (http://host:8766), so a call can be given a link to listen to it.
    pub audio_base: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: 8770,
            bind: "127.0.0.1".into(),
            token: String::new(),
            allowed_networks: "127.0.0.0/8".into(),
            include_encrypted: false,
            days: 30,
            max_audio_mb: 10,
            unit_names_file: String::new(),
            audio_base: String::new(),
        }
    }
}

/// Settings for each system.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemConfig {
    /// Leave this system out
    ///
    /// Its calls aren't indexed, so the assistant never sees them.
    pub skip: bool,
}

impl Config {
    pub fn check(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("the port can't be 0".into());
        }
        if !(1..=365).contains(&self.days) {
            return Err("days is 1 to 365".into());
        }
        if !(1..=100).contains(&self.max_audio_mb) {
            return Err("the largest recording is 1 to 100 MB".into());
        }
        if self.bind.trim().is_empty() {
            return Err("say which address to listen on (127.0.0.1 for this machine only)".into());
        }
        if !crate::http::is_local(&self.bind) && self.token.trim().len() < 16 {
            return Err("to listen beyond this machine, set an access token of at least 16 characters".into());
        }
        crate::net::parse_list(&self.allowed_networks)?;
        Ok(())
    }
}
