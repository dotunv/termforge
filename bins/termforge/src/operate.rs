//! What can be observed about running systems: environment capabilities and
//! listening ports.
//!
//! Plain data and probes with no GPUI or session types, so it can be tested
//! without a display. Everything here is observation: nothing is started,
//! stopped or modified, and a probe that cannot answer says so instead of
//! returning an empty list.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

/// Whether a capability works for an environment, and why not when it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Available,
    Unavailable(&'static str),
}

/// One thing the workbench can or cannot do in an environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub name: &'static str,
    pub support: Support,
}

/// Capabilities of the environment a session runs in. `remote_host` is the
/// SSH host of the active session, or `None` for this machine.
///
/// Remote sessions are terminals only for now: the explorer, Git, service
/// launching and port discovery all read or act on this machine, and the UI
/// says so rather than showing local data under a remote name.
pub fn capabilities(remote_host: Option<&str>, container_runtime: Option<&str>) -> Vec<Capability> {
    let remote = remote_host.is_some();
    let local_only = |support: Support| {
        if remote {
            Support::Unavailable("reads this machine, not the remote host")
        } else {
            support
        }
    };
    vec![
        Capability {
            name: "Terminal sessions",
            support: Support::Available,
        },
        Capability {
            name: "Session output",
            support: Support::Available,
        },
        Capability {
            name: "Files",
            support: local_only(Support::Available),
        },
        Capability {
            name: "Git",
            support: local_only(Support::Available),
        },
        Capability {
            name: "Start and stop services",
            support: if remote {
                Support::Unavailable("services start on this machine, not the remote host")
            } else {
                Support::Available
            },
        },
        Capability {
            name: "Containers",
            support: if remote {
                Support::Unavailable("containers are read from this machine, not the remote host")
            } else if container_runtime.is_some() {
                Support::Available
            } else {
                Support::Unavailable("no docker or podman on PATH")
            },
        },
        Capability {
            name: "Listening ports",
            support: if remote {
                Support::Unavailable("ports are read from this machine, not the remote host")
            } else if cfg!(target_os = "linux") {
                Support::Available
            } else {
                Support::Unavailable("port discovery is implemented for Linux only")
            },
        },
    ]
}

/// How widely a listening socket can be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Bound to a loopback address: only this machine can connect.
    LocalOnly,
    /// Bound to every interface: other machines may be able to connect.
    AllInterfaces,
    /// Bound to one specific non-loopback address.
    Address,
}

impl Reach {
    pub fn of(addr: IpAddr) -> Self {
        if addr.is_loopback() {
            Self::LocalOnly
        } else if addr.is_unspecified() {
            Self::AllInterfaces
        } else {
            Self::Address
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::LocalOnly => "this machine only",
            Self::AllInterfaces => "all interfaces",
            Self::Address => "one interface",
        }
    }
}

/// A TCP socket in the listening state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListeningPort {
    pub port: u16,
    pub addr: IpAddr,
    /// Owning process, when it could be read. Processes of other users are
    /// not readable, so `None` means unknown, not "no owner".
    pub pid: Option<u32>,
    pub process: Option<String>,
    pub cwd: Option<PathBuf>,
    /// The owner's working directory is inside the project. This is the only
    /// association made: it is a heuristic, not proof of who started it.
    pub in_project: bool,
}

impl ListeningPort {
    pub fn reach(&self) -> Reach {
        Reach::of(self.addr)
    }
}

/// The result of asking the operating system for listening ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortProbe {
    Scanned(Vec<ListeningPort>),
    /// The probe cannot run here; the reason is shown to the user.
    Unsupported(String),
}

/// Raw listening-socket row from `/proc/net/tcp` or `/proc/net/tcp6`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSocket {
    pub addr: IpAddr,
    pub port: u16,
    pub inode: u64,
}

/// Parse `/proc/net/tcp` (`v6 == false`) or `/proc/net/tcp6` text, keeping
/// only sockets in the listening state. Malformed rows are skipped.
pub fn parse_proc_net_tcp(text: &str, v6: bool) -> Vec<RawSocket> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // sl, local, remote, state, queues, timer, retransmits, uid, timeout, inode
            if fields.len() < 10 || fields[3] != "0A" {
                return None;
            }
            let (addr, port) = fields[1].split_once(':')?;
            Some(RawSocket {
                addr: parse_proc_addr(addr, v6)?,
                port: u16::from_str_radix(port, 16).ok()?,
                inode: fields[9].parse().ok()?,
            })
        })
        .collect()
}

/// Addresses in `/proc/net/tcp*` are written as native-endian 32-bit words.
/// Linux on every supported architecture is little-endian here.
fn parse_proc_addr(hex: &str, v6: bool) -> Option<IpAddr> {
    if !v6 {
        let word = u32::from_str_radix(hex, 16).ok()?;
        return Some(IpAddr::V4(Ipv4Addr::from(word.swap_bytes())));
    }
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, chunk) in hex.as_bytes().chunks(8).enumerate() {
        let word = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
        bytes[i * 4..i * 4 + 4].copy_from_slice(&word.swap_bytes().to_be_bytes());
    }
    Some(IpAddr::V6(Ipv6Addr::from(bytes)))
}

/// Whether `dir` is `root` or inside it.
pub fn is_within(dir: &Path, root: &Path) -> bool {
    dir.starts_with(root)
}

/// Listening TCP ports on this machine, with the owning process when it is
/// readable. Ports whose owner is in `project_root` are marked `in_project`.
#[cfg(target_os = "linux")]
pub fn scan_ports(project_root: Option<&Path>) -> PortProbe {
    use std::collections::HashMap;

    let mut sockets = Vec::new();
    let mut readable = false;
    for (path, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        if let Ok(text) = std::fs::read_to_string(path) {
            readable = true;
            sockets.extend(parse_proc_net_tcp(&text, v6));
        }
    }
    if !readable {
        return PortProbe::Unsupported("/proc/net/tcp could not be read".into());
    }

    // Map socket inodes to the processes holding them. Only this user's
    // processes can be inspected; the rest stay unattributed.
    let wanted: std::collections::HashSet<u64> = sockets.iter().map(|s| s.inode).collect();
    let mut owners: HashMap<u64, u32> = HashMap::new();
    if let Ok(procs) = std::fs::read_dir("/proc") {
        for entry in procs.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                let Ok(target) = std::fs::read_link(fd.path()) else {
                    continue;
                };
                let target = target.to_string_lossy();
                if let Some(inode) = target
                    .strip_prefix("socket:[")
                    .and_then(|rest| rest.strip_suffix(']'))
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    if wanted.contains(&inode) {
                        owners.entry(inode).or_insert(pid);
                    }
                }
            }
        }
    }

    let mut ports: Vec<ListeningPort> = sockets
        .into_iter()
        .map(|socket| {
            let pid = owners.get(&socket.inode).copied();
            let process = pid.and_then(|pid| {
                std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .ok()
                    .map(|name| name.trim().to_owned())
            });
            let cwd = pid.and_then(|pid| std::fs::read_link(format!("/proc/{pid}/cwd")).ok());
            let in_project = match (&cwd, project_root) {
                (Some(cwd), Some(root)) => is_within(cwd, root),
                _ => false,
            };
            ListeningPort {
                port: socket.port,
                addr: socket.addr,
                pid,
                process,
                cwd,
                in_project,
            }
        })
        .collect();
    ports.sort_by_key(|p| (!p.in_project, p.port, p.addr));
    ports.dedup_by_key(|p| (p.port, p.pid, p.reach()));
    PortProbe::Scanned(ports)
}

#[cfg(not(target_os = "linux"))]
pub fn scan_ports(_project_root: Option<&Path>) -> PortProbe {
    PortProbe::Unsupported("port discovery is implemented for Linux only".into())
}

// ---------------------------------------------------------------- containers

/// Lifecycle state reported by the container runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Running,
    Paused,
    Restarting,
    Created,
    Exited,
    Other(String),
}

impl ContainerState {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "created" | "configured" => Self::Created,
            "exited" | "stopped" | "dead" => Self::Exited,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Restarting => "restarting",
            Self::Created => "created",
            Self::Exited => "exited",
            Self::Other(other) => other,
        }
    }
}

/// A container as reported by `docker ps -a` or `podman ps -a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    /// The runtime's own status text, such as "Up 3 minutes".
    pub status: String,
    pub ports: String,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    pub working_dir: Option<PathBuf>,
    /// Compose started it from a directory inside the project. As with
    /// ports, this is the only association made.
    pub in_project: bool,
}

/// The result of asking the container runtime what exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerProbe {
    /// Neither `docker` nor `podman` is on `PATH`.
    NoRuntime,
    /// A runtime exists but did not answer (daemon stopped, no permission,
    /// or it took too long).
    Unreachable {
        runtime: &'static str,
        message: String,
    },
    Listed {
        runtime: &'static str,
        containers: Vec<Container>,
    },
}

/// Fields requested from the runtime, tab separated. Plain template fields
/// work on both Docker and Podman, unlike their JSON output.
const PS_FORMAT: &str =
    "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.Ports}}\t{{.Labels}}";

/// The container CLI to use, preferring Docker.
pub fn container_runtime() -> Option<&'static str> {
    ["docker", "podman"]
        .into_iter()
        .find(|name| crate::surface::on_path(name))
}

/// Value of `key` in a runtime label dump: Docker prints `k=v,k2=v2`,
/// Podman prints `map[k:v k2:v2]`.
pub fn label_value(labels: &str, key: &str) -> Option<String> {
    let start = labels.find(key)? + key.len();
    let rest = labels[start..].strip_prefix(['=', ':'])?;
    let end = rest.find([',', ']', '}']).unwrap_or(rest.len());
    let value = if labels.starts_with("map[") {
        // Podman separates pairs with spaces.
        rest[..end].split(' ').next().unwrap_or_default()
    } else {
        &rest[..end]
    };
    (!value.is_empty()).then(|| value.to_owned())
}

/// Parse the tab-separated output of `ps -a --format PS_FORMAT`. Rows that do
/// not have all fields are skipped.
pub fn parse_container_list(text: &str, project_root: Option<&Path>) -> Vec<Container> {
    let mut containers: Vec<Container> = text
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 7 || fields[0].is_empty() {
                return None;
            }
            let labels = fields[6];
            let working_dir = label_value(labels, "com.docker.compose.project.working_dir")
                .or_else(|| label_value(labels, "com.docker.compose.project.working_dir"))
                .map(PathBuf::from);
            let in_project = match (&working_dir, project_root) {
                (Some(dir), Some(root)) => is_within(dir, root),
                _ => false,
            };
            Some(Container {
                id: fields[0].to_owned(),
                name: fields[1].trim_start_matches('/').to_owned(),
                image: fields[2].to_owned(),
                state: ContainerState::parse(fields[3]),
                status: fields[4].to_owned(),
                ports: fields[5].to_owned(),
                compose_project: label_value(labels, "com.docker.compose.project"),
                compose_service: label_value(labels, "com.docker.compose.service"),
                working_dir,
                in_project,
            })
        })
        .collect();
    containers.sort_by_key(|c| (!c.in_project, c.name.clone()));
    containers
}

/// Run `command`, giving up and killing it after `limit`. Output is drained
/// on threads so a chatty child cannot block on a full pipe.
pub fn run_with_timeout(
    command: &mut std::process::Command,
    limit: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the runtime did not answer in time",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    Ok(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// List all containers (running or not) known to the runtime.
pub fn list_containers(project_root: Option<&Path>) -> ContainerProbe {
    let Some(runtime) = container_runtime() else {
        return ContainerProbe::NoRuntime;
    };
    let mut command = std::process::Command::new(runtime);
    command.args(["ps", "-a", "--format", PS_FORMAT]);
    match run_with_timeout(&mut command, std::time::Duration::from_secs(5)) {
        Ok(out) if out.status.success() => ContainerProbe::Listed {
            runtime,
            containers: parse_container_list(&String::from_utf8_lossy(&out.stdout), project_root),
        },
        Ok(out) => ContainerProbe::Unreachable {
            runtime,
            message: first_line(&String::from_utf8_lossy(&out.stderr)),
        },
        Err(error) => ContainerProbe::Unreachable {
            runtime,
            message: error.to_string(),
        },
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no error message")
        .to_owned()
}

/// A lifecycle change the user asked for on one container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
}

impl ContainerAction {
    pub const fn progressive(self) -> &'static str {
        match self {
            Self::Start => "starting",
            Self::Stop => "stopping",
            Self::Restart => "restarting",
        }
    }

    pub const fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// Whether `id` can be passed to a shell or CLI without quoting. Container
/// ids are hexadecimal, so anything else is refused rather than escaped.
pub fn is_safe_container_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Run one lifecycle action. Returns the runtime's error text on failure.
pub fn run_container_action(action: ContainerAction, id: &str) -> Result<(), String> {
    if !is_safe_container_id(id) {
        return Err("unexpected container id".into());
    }
    let runtime = container_runtime().ok_or("no docker or podman on PATH")?;
    let mut command = std::process::Command::new(runtime);
    command.args([action.verb(), id]);
    match run_with_timeout(&mut command, std::time::Duration::from_secs(40)) {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(first_line(&String::from_utf8_lossy(&out.stderr))),
        Err(error) => Err(error.to_string()),
    }
}

/// Compose and container definition files found directly in `root`.
pub fn container_files(root: &Path) -> Vec<&'static str> {
    [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
        "Dockerfile",
        "Containerfile",
    ]
    .into_iter()
    .filter(|name| root.join(name).is_file())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0000000000000000 100 0 0 10 0
   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 22222 1 0000000000000000 100 0 0 10 0
   2: 0100007F:9C40 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 33333 1 0000000000000000 20 4 30 10 -1
   3: garbage
";

    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 44444 1 0000000000000000 100 0 0 10 0
   1: 00000000000000000000000000000000:1F40 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 55555 1 0000000000000000 100 0 0 10 0
";

    #[test]
    fn ipv4_listeners_are_decoded_and_other_states_skipped() {
        let sockets = parse_proc_net_tcp(TCP, false);
        assert_eq!(sockets.len(), 2, "established and malformed rows skipped");
        assert_eq!(sockets[0].addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(sockets[0].port, 8080);
        assert_eq!(sockets[0].inode, 11111);
        assert_eq!(sockets[1].addr, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(sockets[1].port, 22);
    }

    #[test]
    fn ipv6_listeners_are_decoded() {
        let sockets = parse_proc_net_tcp(TCP6, true);
        assert_eq!(sockets.len(), 2);
        assert_eq!(sockets[0].addr, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(sockets[0].port, 3000);
        assert_eq!(sockets[1].addr, IpAddr::V6(Ipv6Addr::UNSPECIFIED));
        assert_eq!(sockets[1].port, 8000);
    }

    #[test]
    fn reach_distinguishes_loopback_from_exposed_sockets() {
        assert_eq!(Reach::of(Ipv4Addr::LOCALHOST.into()), Reach::LocalOnly);
        assert_eq!(Reach::of(Ipv6Addr::LOCALHOST.into()), Reach::LocalOnly);
        assert_eq!(
            Reach::of(Ipv4Addr::UNSPECIFIED.into()),
            Reach::AllInterfaces
        );
        assert_eq!(
            Reach::of(Ipv4Addr::new(192, 168, 1, 5).into()),
            Reach::Address
        );
    }

    #[test]
    fn project_association_is_by_working_directory() {
        let root = Path::new("/home/me/app");
        assert!(is_within(Path::new("/home/me/app"), root));
        assert!(is_within(Path::new("/home/me/app/web"), root));
        assert!(!is_within(Path::new("/home/me/app2"), root));
        assert!(!is_within(Path::new("/tmp"), root));
    }

    #[test]
    fn remote_environments_say_what_they_cannot_do() {
        let local = capabilities(None, Some("docker"));
        let remote = capabilities(Some("build-box"), Some("docker"));
        assert_eq!(local.len(), remote.len());
        let find = |caps: &[Capability], name: &str| {
            caps.iter()
                .find(|c| c.name == name)
                .unwrap()
                .support
                .clone()
        };
        assert_eq!(find(&local, "Files"), Support::Available);
        for name in [
            "Files",
            "Git",
            "Start and stop services",
            "Containers",
            "Listening ports",
        ] {
            assert!(
                matches!(find(&remote, name), Support::Unavailable(_)),
                "{name} must be unavailable remotely"
            );
        }
        assert_eq!(find(&remote, "Terminal sessions"), Support::Available);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_scan_finds_a_port_we_open() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cwd = std::env::current_dir().unwrap();
        let PortProbe::Scanned(ports) = scan_ports(Some(&cwd)) else {
            panic!("linux scan must run");
        };
        let ours = ports.iter().find(|p| p.port == port).expect("our port");
        assert_eq!(ours.reach(), Reach::LocalOnly);
        assert_eq!(ours.pid, Some(std::process::id()));
        assert!(ours.in_project, "the test process runs inside its cwd");
    }

    #[test]
    fn docker_container_rows_are_parsed_with_compose_association() {
        let text = "abc123\tweb-1\tnginx:1\trunning\tUp 3 minutes\t0.0.0.0:8080->80/tcp\tcom.docker.compose.project=app,com.docker.compose.service=web,com.docker.compose.project.working_dir=/home/me/app\n\
                    def456\t/db\tpostgres\texited\tExited (0) 2 hours ago\t\tmaintainer=x\n\
                    broken\tonly-two\n";
        let containers = parse_container_list(text, Some(Path::new("/home/me/app")));
        assert_eq!(containers.len(), 2);
        let web = &containers[0];
        assert!(web.in_project, "project containers sort first");
        assert_eq!(web.name, "web-1");
        assert_eq!(web.state, ContainerState::Running);
        assert_eq!(web.compose_project.as_deref(), Some("app"));
        assert_eq!(web.compose_service.as_deref(), Some("web"));
        let db = &containers[1];
        assert!(!db.in_project);
        assert_eq!(db.name, "db");
        assert_eq!(db.state, ContainerState::Exited);
        assert_eq!(db.compose_project, None);
    }

    #[test]
    fn podman_label_maps_are_understood() {
        let labels = "map[com.docker.compose.project:app com.docker.compose.project.working_dir:/srv/app io.x:y]";
        assert_eq!(
            label_value(labels, "com.docker.compose.project").as_deref(),
            Some("app")
        );
        assert_eq!(
            label_value(labels, "com.docker.compose.project.working_dir").as_deref(),
            Some("/srv/app")
        );
        assert_eq!(label_value(labels, "missing"), None);
    }

    #[test]
    fn container_ids_are_never_passed_through_unsafe() {
        assert!(is_safe_container_id("0123abcDEF"));
        assert!(!is_safe_container_id(""));
        assert!(!is_safe_container_id("a; rm -rf ~"));
        assert!(!is_safe_container_id("$(x)"));
        assert!(run_container_action(ContainerAction::Stop, "a b").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_runtime_is_killed_at_the_deadline() {
        let started = std::time::Instant::now();
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        let result = run_with_timeout(&mut command, std::time::Duration::from_millis(200));
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_chatty_child_does_not_deadlock_the_timeout_runner() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "head -c 300000 /dev/zero | tr '\\0' x"]);
        let out = run_with_timeout(&mut command, std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(out.stdout.len(), 300_000);
    }
}
