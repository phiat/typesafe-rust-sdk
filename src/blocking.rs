//! A blocking client, for synchronous code such as a game loop or a CLI.
//!
//! It has the same methods as the async [`Client`](crate::Client). Like
//! `reqwest::blocking`, it must not be used from inside an async runtime. Run
//! calls on a worker thread when the caller must not stall, such as a frame
//! loop.

use serde::Serialize;

use crate::config::{CallOptions, Config};
use crate::error::Error;
use crate::question::Questions;
use crate::request::{self, Prepared, Reply, Retries, Success};
use crate::response::{Model, Response};

/// A blocking client for the System One API.
///
/// Cloning is cheap and shares the connection pool.
///
/// ```no_run
/// use typesafe::blocking::Client;
/// use typesafe::{Noul, Questions};
///
/// let client = Client::new(); // reads TYPESAFE_API_KEY
/// let questions = Questions::new().ask("billing", Noul::new("Is this ticket about billing?"));
/// let response = client.system_one("I was charged twice!", &questions)?;
/// println!("{:.2}", response.noul("billing").unwrap().noul);
/// # Ok::<(), typesafe::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct Client {
    config: Config,
    http: reqwest::blocking::Client,
}

impl Default for Client {
    fn default() -> Self {
        Client::new()
    }
}

impl Client {
    /// A client configured from the environment ([`Config::from_env`]).
    ///
    /// # Panics
    ///
    /// If the TLS backend can't be initialized, or if called from inside an
    /// async runtime, as `reqwest::blocking::Client::new` does.
    /// [`Client::with_config`] returns the first as an error instead.
    pub fn new() -> Self {
        Client::with_config(Config::from_env()).expect("the HTTP client could not be built")
    }

    pub fn with_config(config: Config) -> Result<Self, Error> {
        let http = reqwest::blocking::Client::builder()
            .build()
            .map_err(Error::from_reqwest)?;
        Ok(Client { config, http })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Whether the client has an API key.
    pub fn is_configured(&self) -> bool {
        self.config.is_configured()
    }

    /// Asks `questions` about `state` in one request.
    ///
    /// `state` is a string, or anything that serializes to a JSON object or
    /// array. A struct keeps its field order on the wire.
    pub fn system_one<S: Serialize + ?Sized>(
        &self,
        state: &S,
        questions: &Questions,
    ) -> Result<Response, Error> {
        self.system_one_with(state, questions, &CallOptions::default())
    }

    /// [`system_one`](Client::system_one) with per-call overrides.
    pub fn system_one_with<S: Serialize + ?Sized>(
        &self,
        state: &S,
        questions: &Questions,
        options: &CallOptions,
    ) -> Result<Response, Error> {
        let prepared = request::system_one(&self.config, state, questions, options)?;
        let success = self.execute(prepared)?;
        request::decode_system_one(success, questions)
    }

    /// The model names the account can send, aliases included. Versioned ids
    /// such as `jev-1.13.0` are accepted whether or not they're listed.
    pub fn models(&self) -> Result<Vec<Model>, Error> {
        let prepared = request::models(&self.config)?;
        let success = self.execute(prepared)?;
        request::decode_models(success)
    }

    fn execute(&self, prepared: Prepared) -> Result<Success, Error> {
        let mut retries = Retries::start(prepared.retry.clone());
        loop {
            retries.attempts += 1;
            let result = self.attempt(&prepared, &retries);
            match retries.after(&result) {
                Some(delay) => std::thread::sleep(delay),
                None => return request::finish(result),
            }
        }
    }

    fn attempt(&self, prepared: &Prepared, retries: &Retries) -> Result<Reply, Error> {
        let mut builder = self
            .http
            .request(prepared.method.clone(), &prepared.url)
            .headers(prepared.headers.clone())
            .timeout(prepared.timeout);
        if let Some((name, value)) = retries.retry_count_header() {
            builder = builder.header(name, value);
        }
        if let Some(body) = &prepared.body {
            builder = builder.body(body.clone());
        }
        let response = builder.send().map_err(Error::from_reqwest)?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().map_err(Error::from_reqwest)?;
        Ok(Reply {
            status,
            headers,
            body: body.to_vec(),
        })
    }
}
