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
pub fn capabilities(remote_host: Option<&str>) -> Vec<Capability> {
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
        let local = capabilities(None);
        let remote = capabilities(Some("build-box"));
        assert_eq!(local.len(), remote.len());
        let find = |caps: &[Capability], name: &str| {
            caps.iter()
                .find(|c| c.name == name)
                .unwrap()
                .support
                .clone()
        };
        assert_eq!(find(&local, "Files"), Support::Available);
        for name in ["Files", "Git", "Start and stop services", "Listening ports"] {
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
}
