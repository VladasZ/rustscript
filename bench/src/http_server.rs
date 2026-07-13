use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

pub struct HttpServer {
    child: Option<Child>,
    url: String,
}

impl HttpServer {
    pub fn start(program: &Path) -> Result<Self> {
        let mut child = Command::new(program).stdout(Stdio::piped()).spawn()?;
        let stdout = child.stdout.take().context("HTTP server has no stdout")?;
        let mut reader = BufReader::new(stdout);
        let mut url = String::new();
        reader.read_line(&mut url)?;
        let url = url.trim().to_string();
        if url.is_empty() {
            bail!("HTTP server did not report its URL");
        }
        Ok(Self {
            child: Some(child),
            url,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn stop(mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            terminate(&mut child)?;
        }
        Ok(())
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child
            && let Err(error) = terminate(child)
        {
            eprintln!("could not stop benchmark HTTP server: {error}");
        }
    }
}

fn terminate(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        child.kill()?;
        child.wait()?;
    }
    Ok(())
}
