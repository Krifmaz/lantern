// Ported from autobrr v1.90.0 (internal/announce/announce.go), GPL-2.0-or-later.
//! Reading announces: one queue per announce channel of an indexer. Each queue takes the channel's lines in the
//! order they come and reads them with the channel's line patterns, one announce at a time (a multi-line
//! announce takes as many lines as it has patterns, in order). A line that doesn't match drops the announce it
//! was part of. A complete announce becomes a release ([`super::definition::Parse::parse`]) that is handed to the
//! filters at once.
//!
//! The definitions themselves live in [`super::definition`]; [`Library`] is re-exported here for the callers that
//! knew it under this module.

use super::definition::{IndexerDefinition, ParseLine};
use super::release::{IndexerMinimal, Release};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::mpsc;

pub use super::definition::Library;

/// Where finished releases go (the filters).
pub type Sink = Arc<dyn Fn(Release) + Send + Sync>;

/// How many lines a channel's queue holds before the reader waits (autobrr's 128).
const QUEUE: usize = 128;

/// The queues of one indexer's announce channels (autobrr's `announceProcessor`).
pub struct Processor {
    queues: HashMap<String, mpsc::Sender<String>>,
    tasks: Vec<tokio::task::AbortHandle>,
}

impl Processor {
    /// Queues for every announce channel of `def` (a definition mapped with its indexer's settings); finished
    /// releases go to `sink`.
    pub fn new(def: Arc<IndexerDefinition>, sink: Sink) -> Processor {
        let mut queues = HashMap::new();
        let mut tasks = Vec::new();
        for c in def.irc.iter().flat_map(|i| i.channels.iter()) {
            let name = c.name.to_lowercase();
            let (tx, rx) = mpsc::channel(QUEUE);
            queues.insert(name.clone(), tx);
            let task = tokio::spawn(consume(def.clone(), name, rx, sink.clone()));
            tasks.push(task.abort_handle());
        }
        Processor { queues, tasks }
    }

    /// Queues `line` of `channel` (autobrr's `AddLineToQueue`); the problem in autobrr's words.
    pub async fn add_line(&self, channel: &str, line: &str) -> Result<(), String> {
        let channel = channel.to_lowercase();
        let q = self.queues.get(&channel).ok_or_else(|| format!("no queue for channel ({channel}) found"))?;
        q.send(line.to_string()).await.map_err(|_| "could not queue line".to_string())
    }
}

impl Drop for Processor {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// A new release of indexer `def`, before an announce fills it in.
fn new_release(def: &IndexerDefinition) -> Release {
    let mut r = Release::new(IndexerMinimal {
        id: def.id,
        name: def.name.clone(),
        identifier: def.identifier.clone(),
        identifier_external: def.identifier_external.clone(),
    });
    r.protocol = def.protocol.clone();
    r
}

/// Reads channel `channel`'s queue, announce by announce (autobrr's `processQueue`).
async fn consume(def: Arc<IndexerDefinition>, channel: String, mut rx: mpsc::Receiver<String>, sink: Sink) {
    let Some(parse) = def.irc.as_ref().and_then(|i| i.channel(&channel)).and_then(|c| c.parse.clone()) else {
        tracing::error!("announce: channel {channel} of {} has no parse configuration", def.name);
        return;
    };
    loop {
        let mut vars = BTreeMap::new();
        let mut failed = false;
        for pl in &parse.lines {
            let Some(line) = rx.recv().await else { return };
            if !matches!(pl.parse_line(&mut vars, &line, pl.ignore), Ok(true)) {
                tracing::debug!("announce: a line on {channel} didn't match {}'s pattern", def.name);
                failed = true;
                break;
            }
        }
        if failed {
            continue;
        }
        let mut r = new_release(&def);
        match parse.parse(&def, &channel, &mut vars, &mut r) {
            Ok(()) => sink(r),
            Err(e) => tracing::warn!("announce: {} couldn't read an announce on {channel}: {e}", def.name),
        }
    }
}

/// Reads the complete announce `lines` of channel `channel` at once (the first channel when empty), for an
/// announce sent by hand; the release or the problem in words.
pub fn read_lines(def: &IndexerDefinition, channel: &str, lines: &[&str]) -> Result<Release, String> {
    let irc = def.irc.as_ref().ok_or_else(|| format!("{} doesn't announce on IRC", def.name))?;
    let ch = if channel.is_empty() { irc.channels.first() } else { irc.channel(channel) };
    let ch = ch.ok_or_else(|| format!("{} has no announce channel {channel}", def.name))?;
    let parse = ch.parse.as_ref().ok_or_else(|| format!("{} doesn't say how to read {}", def.name, ch.name))?;
    if lines.len() < parse.lines.len() {
        return Err(format!("An announce of {} has {} lines", def.name, parse.lines.len()));
    }
    let mut vars = BTreeMap::new();
    for (pl, line) in parse.lines.iter().zip(lines) {
        if !ParseLine::parse_line(pl, &mut vars, line, pl.ignore)? {
            return Err(format!("The line doesn't look like an announce of {}", def.name));
        }
    }
    let mut r = new_release(def);
    parse.parse(def, &ch.name, &mut vars, &mut r)?;
    Ok(r)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::definition::parse_definition;
    use super::*;

    /// A multi-line definition in autobrr's format, written for these tests.
    pub const MULTI: &str = r##"
version: 2
identifier: harbour
name: Harbour
urls: [https://harbour.example/]
protocol: torrent
settings:
  - { name: passkey, type: secret, required: true, label: Passkey }
irc:
  network: Harbour
  server: irc.harbour.example
  port: 6667
  channels:
    - name: "#New"
      announcers: [Bot]
      parse:
        type: multi
        forcesizeunit: MB
        lines:
          - pattern: 'Name: (.+)'
            vars: [torrentName]
          - pattern: 'Size: (\d+) Tags: (.*)'
            vars: [torrentSize, tags]
          - pattern: 'Link: (https?://.+/)t/(\d+)'
            vars: [baseUrl, torrentId]
        match:
          downloadurl: "/get/{{ .torrentId }}?k={{ .passkey }}"
"##;

    #[tokio::test]
    async fn reads_a_multi_line_announce_in_order() {
        let mut d = parse_definition(MULTI).unwrap();
        d.id = 4;
        d.settings_map.insert("passkey".into(), "abc".into());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let p = Processor::new(Arc::new(d), Arc::new(move |r| drop(tx.send(r))));
        // a broken announce: the second line doesn't match, so the third starts the next announce
        p.add_line("#new", "Name: Old.Release-X").await.unwrap();
        p.add_line("#NEW", "someone says hi").await.unwrap();
        for l in ["Name: Movie.2024.2160p.BluRay-GRP", "Size: 2048 Tags: hdr, action", "Link: https://harbour.example/t/9"] {
            p.add_line("#new", l).await.unwrap();
        }
        let r = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert_eq!(r.torrent_name, "Movie.2024.2160p.BluRay-GRP");
        assert_eq!(r.size, 2_048_000_000);
        assert_eq!(r.tags, vec!["hdr", "action"]);
        assert_eq!(r.download_url, "https://harbour.example/get/9?k=abc");
        assert_eq!(r.indexer.id, 4);
        assert!(p.add_line("#other", "x").await.unwrap_err().contains("no queue for channel (#other) found"));
    }
}
