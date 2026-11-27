//! Docker scanning: every image, stopped container, unused volume and the
//! build cache become individual, selectable items.

use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct Df {
    images: Option<Vec<RawImage>>,
    containers: Option<Vec<RawContainer>>,
    volumes: Option<Vec<RawVolume>>,
    build_cache: Option<Vec<RawCache>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct RawImage {
    #[serde(rename = "ID")]
    id: String,
    repository: String,
    tag: String,
    containers: String,
    created_since: String,
    unique_size: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct RawContainer {
    #[serde(rename = "ID")]
    id: String,
    names: String,
    image: String,
    size: String,
    state: String,
    status: String,
    running_for: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct RawVolume {
    name: String,
    links: String,
    size: String,
    labels: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct RawCache {
    size: String,
    in_use: serde_json::Value,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    // Deletion order matters: containers first so their images become free.
    Container,
    Image,
    Volume,
    BuildCache,
}

/// How risky an item is to delete, which decides what `--yes` removes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Dangling images, build cache, stopped containers (what `docker system prune` removes).
    Safe,
    /// Tagged images not used by any container (`--docker-all`).
    Unused,
    /// Volumes not attached to any container (`--docker-volumes`). May hold data!
    Volume,
    /// Used by a container: never removed automatically.
    InUse,
}

pub struct Obj {
    pub kind: Kind,
    pub tier: Tier,
    /// What we pass to the docker CLI to delete it.
    pub target: String,
    pub label: String,
    pub age: String,
    pub size: u64,
}

impl Obj {
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            Kind::Image => "docker image",
            Kind::Container => "docker ctr",
            Kind::Volume => "docker vol",
            Kind::BuildCache => "docker cache",
        }
    }

    /// Pre-selected in the interactive picker: only things that are pure waste.
    pub fn default_selected(&self) -> bool {
        self.tier == Tier::Safe && self.kind != Kind::Container
    }

    /// Selected by `clean --yes`.
    pub fn auto_selected(&self, all: bool, volumes: bool) -> bool {
        match self.tier {
            Tier::Safe => true,
            Tier::Unused => all,
            Tier::Volume => volumes,
            Tier::InUse => false,
        }
    }

    pub fn delete(&self) -> Result<()> {
        let args: Vec<&str> = match self.kind {
            Kind::Container => vec!["rm", &self.target],
            Kind::Image => vec!["rmi", &self.target],
            Kind::Volume => vec!["volume", "rm", &self.target],
            Kind::BuildCache => vec!["builder", "prune", "-af"],
        };
        let out = Command::new("docker").args(&args).output()?;
        if !out.status.success() {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }
}

/// True when the docker CLI exists and the daemon is reachable.
pub fn available() -> bool {
    Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Docker prints decimal (SI) sizes: "1.108GB", "462.9kB", "2B", "N/A".
pub fn parse_docker_size(s: &str) -> u64 {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let Ok(n) = num.parse::<f64>() else { return 0 };
    let mult = match unit.trim().to_ascii_lowercase().as_str() {
        "b" | "" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => 0.0,
    };
    (n * mult) as u64
}

/// "26 hours ago" -> "26h", "2 weeks ago" -> "2w", "About an hour ago" -> "1h".
pub fn compact_age(s: &str) -> String {
    let s = s.trim().trim_end_matches(" ago").to_ascii_lowercase();
    let mut parts = s.split_whitespace();
    let (Some(n), Some(unit)) = (parts.next(), parts.next()) else {
        return "?".into();
    };
    let n = match n {
        "a" | "an" | "about" => "1",
        "less" => return "now".into(),
        n => n,
    };
    // "About an hour" -> unit is "an"; look one further.
    let unit = if unit == "an" || unit == "a" {
        parts.next().unwrap_or("")
    } else {
        unit
    };
    let u = match unit.trim_end_matches('s') {
        "second" => return "now".into(),
        "minute" => "m",
        "hour" => "h",
        "day" => "d",
        "week" => "w",
        "month" => "mo",
        "year" => "y",
        _ => return "?".into(),
    };
    format!("{n}{u}")
}

fn compose_project(labels: &str) -> Option<&str> {
    labels
        .split(',')
        .find_map(|l| l.strip_prefix("com.docker.compose.project="))
}

pub fn scan() -> Result<Vec<Obj>> {
    let out = Command::new("docker")
        .args(["system", "df", "-v", "--format", "{{json .}}"])
        .output()
        .context("failed to run `docker system df -v`")?;
    if !out.status.success() {
        bail!(
            "docker system df failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let df: Df = serde_json::from_slice(&out.stdout).context("unexpected docker output")?;
    let mut objs = Vec::new();

    for c in df.containers.unwrap_or_default() {
        if matches!(c.state.as_str(), "running" | "restarting" | "paused") {
            continue;
        }
        objs.push(Obj {
            kind: Kind::Container,
            tier: Tier::Safe,
            label: format!("{} ({}) · {}", c.names, c.image, c.status),
            age: compact_age(&c.running_for),
            size: parse_docker_size(&c.size),
            target: c.id,
        });
    }

    for i in df.images.unwrap_or_default() {
        let dangling = i.repository == "<none>";
        let used: u32 = i.containers.parse().unwrap_or(0);
        let name = if dangling {
            format!(
                "<dangling> {}",
                i.id.trim_start_matches("sha256:")
                    .get(..12)
                    .unwrap_or(&i.id)
            )
        } else {
            format!("{}:{}", i.repository, i.tag)
        };
        let (tier, note) = match (used, dangling) {
            (0, true) => (Tier::Safe, String::new()),
            (0, false) => (Tier::Unused, " · unused".to_string()),
            (n, _) => (Tier::InUse, format!(" · used by {n} container(s)")),
        };
        objs.push(Obj {
            kind: Kind::Image,
            tier,
            target: if dangling || i.tag == "<none>" {
                i.id
            } else {
                name.clone()
            },
            label: format!("{name}{note}"),
            age: compact_age(&i.created_since),
            size: parse_docker_size(&i.unique_size),
        });
    }

    for v in df.volumes.unwrap_or_default() {
        if v.links != "0" {
            continue;
        }
        let owner = compose_project(&v.labels)
            .map(|p| format!(" · compose project '{p}'"))
            .unwrap_or_default();
        objs.push(Obj {
            kind: Kind::Volume,
            tier: Tier::Volume,
            label: format!("{}{owner} · unused, may hold data", v.name),
            age: "-".into(),
            size: parse_docker_size(&v.size),
            target: v.name,
        });
    }

    let caches = df.build_cache.unwrap_or_default();
    let cache_size: u64 = caches
        .iter()
        .filter(|c| !matches!(&c.in_use, serde_json::Value::Bool(true)) && c.in_use != "true")
        .map(|c| parse_docker_size(&c.size))
        .sum();
    if cache_size > 0 {
        objs.push(Obj {
            kind: Kind::BuildCache,
            tier: Tier::Safe,
            target: String::new(),
            label: format!("Docker build cache ({} entries)", caches.len()),
            age: "-".into(),
            size: cache_size,
        });
    }
    Ok(objs)
}

pub struct DockerRow {
    pub kind: String,
    pub size: String,
    pub reclaimable: String,
}

/// Summary table from `docker system df`.
pub fn usage() -> Result<Vec<DockerRow>> {
    let out = Command::new("docker")
        .args([
            "system",
            "df",
            "--format",
            "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}",
        ])
        .output()
        .context("failed to run `docker system df`")?;
    if !out.status.success() {
        bail!(
            "docker system df failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut p = line.split('\t');
            Some(DockerRow {
                kind: p.next()?.to_string(),
                size: p.next()?.to_string(),
                reclaimable: p.next()?.to_string(),
            })
        })
        .collect())
}

/// Bulk prune, like `docker system prune`. `all` also removes unused (not
/// just dangling) images; `volumes` also removes unused volumes.
pub fn prune(all: bool, volumes: bool) -> Result<()> {
    let mut sys = vec!["system", "prune", "-f"];
    if all {
        sys.push("-a");
    }
    if volumes {
        sys.push("--volumes");
    }
    run(&sys)?;
    let mut builder = vec!["builder", "prune", "-f"];
    if all {
        builder.push("-a");
    }
    run(&builder)
}

fn run(args: &[&str]) -> Result<()> {
    println!("$ docker {}", args.join(" "));
    let status = Command::new("docker")
        .args(args)
        .status()
        .with_context(|| format!("failed to run docker {}", args.join(" ")))?;
    if !status.success() {
        bail!("docker {} exited with {status}", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_docker_size("2B"), 2);
        assert_eq!(parse_docker_size("462.9kB"), 462_900);
        assert_eq!(parse_docker_size("1.108GB"), 1_108_000_000);
        assert_eq!(parse_docker_size("N/A"), 0);
    }

    #[test]
    fn ages() {
        assert_eq!(compact_age("26 hours ago"), "26h");
        assert_eq!(compact_age("2 weeks ago"), "2w");
        assert_eq!(compact_age("About an hour ago"), "1h");
        assert_eq!(compact_age("6 days ago"), "6d");
        assert_eq!(compact_age("3 months ago"), "3mo");
        assert_eq!(compact_age("Less than a second ago"), "now");
    }

    #[test]
    fn parses_df_json() {
        let json = r#"{"Images":[{"Containers":"0","CreatedSince":"2 weeks ago","ID":"sha256:abc","Repository":"pgvector/pgvector","Tag":"pg18","UniqueSize":"678.3MB"},
            {"Containers":"0","CreatedSince":"1 day ago","ID":"sha256:0123456789abcdef","Repository":"<none>","Tag":"<none>","UniqueSize":"10MB"}],
            "Containers":[{"ID":"c1","Names":"web-1","Image":"web","Size":"274kB","State":"exited","Status":"Exited (0) 6 days ago","RunningFor":"6 days ago"},
            {"ID":"c2","Names":"db-1","Image":"db","Size":"1kB","State":"running","Status":"Up","RunningFor":"1 day ago"}],
            "Volumes":[{"Name":"proj_data","Links":"0","Size":"215.2MB","Labels":"com.docker.compose.project=proj"}],
            "BuildCache":null}"#;
        let df: Df = serde_json::from_str(json).unwrap();
        assert_eq!(df.images.unwrap().len(), 2);
        assert_eq!(df.containers.unwrap().len(), 2);
        assert_eq!(df.volumes.unwrap()[0].name, "proj_data");
        assert!(df.build_cache.is_none());
    }
}
