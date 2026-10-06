//! The computer the engine runs on (ADR-0025): system, processor, graphics
//! (NVIDIA, Vulkan) and memory; and which llama.cpp package fits it.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Windows,
    Macos,
    Linux,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X64,
    Arm64,
    Other,
}

/// How the engine computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// The processor only.
    Cpu,
    /// Any graphics card with a Vulkan driver (AMD, Intel, NVIDIA).
    Vulkan,
    /// NVIDIA cards.
    Cuda,
    /// Apple Silicon.
    Metal,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Cpu => "processador",
            Backend::Vulkan => "Vulkan",
            Backend::Cuda => "NVIDIA (CUDA)",
            Backend::Metal => "Apple (Metal)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct System {
    pub os: Os,
    pub arch: Arch,
    pub nvidia: bool,
    pub vulkan: bool,
    /// Total memory (RAM), when known.
    pub memory_bytes: Option<u64>,
}

impl System {
    pub fn detect() -> Self {
        let os = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        };
        let arch = if cfg!(target_arch = "x86_64") {
            Arch::X64
        } else if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::Other
        };
        Self {
            os,
            arch,
            nvidia: has_nvidia(),
            vulkan: has_vulkan(),
            memory_bytes: total_memory(),
        }
    }

    /// The packages that exist for this system, best first.
    pub fn backends(&self) -> Vec<Backend> {
        match (self.os, self.arch) {
            (Os::Windows, Arch::X64) | (Os::Linux, Arch::X64) => {
                vec![Backend::Cuda, Backend::Vulkan, Backend::Cpu]
            }
            (Os::Windows, Arch::Arm64) | (Os::Linux, Arch::Arm64) => {
                vec![Backend::Vulkan, Backend::Cpu]
            }
            (Os::Macos, Arch::Arm64) => vec![Backend::Metal],
            (Os::Macos, Arch::X64) => vec![Backend::Cpu],
            _ => Vec::new(),
        }
    }

    /// The package chosen when the user does not choose (ADR-0025).
    pub fn auto_backend(&self) -> Option<Backend> {
        match (self.os, self.arch) {
            (Os::Windows, Arch::X64) if self.nvidia => Some(Backend::Cuda),
            (Os::Windows, Arch::X64) => Some(Backend::Vulkan),
            (Os::Windows, Arch::Arm64) => Some(Backend::Cpu),
            (Os::Linux, Arch::X64 | Arch::Arm64) if self.vulkan => Some(Backend::Vulkan),
            (Os::Linux, Arch::X64 | Arch::Arm64) => Some(Backend::Cpu),
            (Os::Macos, Arch::Arm64) => Some(Backend::Metal),
            (Os::Macos, Arch::X64) => Some(Backend::Cpu),
            _ => None,
        }
    }

    /// Context a new model gets: 32k with 24 GB or more, else 16k.
    pub fn default_context(&self) -> u32 {
        match self.memory_bytes {
            Some(bytes) if bytes >= 24 * GIB => 32_768,
            _ => 16_384,
        }
    }

    /// Names of the release files for `backend` (the engine, then extra
    /// libraries such as CUDA's), matched against what a release has.
    pub fn asset_patterns(&self, backend: Backend, tag: &str) -> Option<Vec<AssetPattern>> {
        let arch = match self.arch {
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
            Arch::Other => return None,
        };
        let exact = |name: String| AssetPattern::Exact(name);
        Some(match (self.os, backend) {
            (Os::Windows, Backend::Cpu) => {
                vec![exact(format!("llama-{tag}-bin-win-cpu-{arch}.zip"))]
            }
            (Os::Windows, Backend::Vulkan) => {
                vec![exact(format!("llama-{tag}-bin-win-vulkan-{arch}.zip"))]
            }
            (Os::Windows, Backend::Cuda) => vec![
                AssetPattern::Cuda {
                    prefix: format!("llama-{tag}-bin-win-cuda-"),
                    suffix: format!("-{arch}.zip"),
                },
                AssetPattern::CudaRuntime {
                    prefix: "cudart-llama-bin-win-cuda-".into(),
                    suffix: format!("-{arch}.zip"),
                },
            ],
            (Os::Macos, Backend::Metal | Backend::Cpu) => {
                vec![exact(format!("llama-{tag}-bin-macos-{arch}.tar.gz"))]
            }
            (Os::Linux, Backend::Cpu) => {
                vec![exact(format!("llama-{tag}-bin-ubuntu-{arch}.tar.gz"))]
            }
            (Os::Linux, Backend::Vulkan) => {
                vec![exact(format!(
                    "llama-{tag}-bin-ubuntu-vulkan-{arch}.tar.gz"
                ))]
            }
            (Os::Linux, Backend::Cuda) => vec![
                AssetPattern::Cuda {
                    prefix: format!("llama-{tag}-bin-ubuntu-cuda-"),
                    suffix: format!("-{arch}.tar.gz"),
                },
                AssetPattern::CudaRuntime {
                    prefix: format!("cudart-llama-{tag}-bin-ubuntu-cuda-"),
                    suffix: format!("-{arch}.tar.gz"),
                },
            ],
            _ => return None,
        })
    }
}

pub const GIB: u64 = 1024 * 1024 * 1024;

/// A file of a llama.cpp release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetPattern {
    Exact(String),
    /// `<prefix><cuda version><suffix>`: CUDA 12 is preferred (older
    /// drivers run it); the version found is used for the runtime.
    Cuda {
        prefix: String,
        suffix: String,
    },
    /// The CUDA libraries, for the same version as the engine.
    CudaRuntime {
        prefix: String,
        suffix: String,
    },
}

/// Picks the release files for `patterns` among `names`. CUDA builds come
/// in more than one version: 12.x is preferred, and the runtime libraries
/// must be of the same version.
pub fn pick_assets(patterns: &[AssetPattern], names: &[String]) -> Result<Vec<String>, String> {
    let mut picked = Vec::new();
    let mut cuda: Option<String> = None;
    for pattern in patterns {
        match pattern {
            AssetPattern::Exact(name) => {
                if !names.contains(name) {
                    return Err(format!("o release não tem o arquivo {name}"));
                }
                picked.push(name.clone());
            }
            AssetPattern::Cuda { prefix, suffix } => {
                let mut versions: Vec<(String, String)> = names
                    .iter()
                    .filter_map(|n| {
                        let v = n
                            .strip_prefix(prefix.as_str())?
                            .strip_suffix(suffix.as_str())?;
                        Some((v.to_owned(), n.clone()))
                    })
                    .collect();
                // 12.x first (wider driver support), then the newest.
                versions.sort_by(|a, b| {
                    let twelve = |v: &str| v.starts_with("12.");
                    twelve(&b.0)
                        .cmp(&twelve(&a.0))
                        .then_with(|| version_key(&b.0).cmp(&version_key(&a.0)))
                });
                let (version, name) = versions
                    .into_iter()
                    .next()
                    .ok_or("o release não tem o pacote CUDA para este sistema")?;
                cuda = Some(version);
                picked.push(name);
            }
            AssetPattern::CudaRuntime { prefix, suffix } => {
                let version = cuda.as_deref().ok_or("versão do CUDA desconhecida")?;
                let name = format!("{prefix}{version}{suffix}");
                if !names.contains(&name) {
                    return Err(format!("o release não tem as bibliotecas do CUDA ({name})"));
                }
                picked.push(name);
            }
        }
    }
    Ok(picked)
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').filter_map(|p| p.parse().ok()).collect()
}

fn has_nvidia() -> bool {
    if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        Path::new(&root)
            .join("System32")
            .join("nvcuda.dll")
            .exists()
    } else if cfg!(target_os = "linux") {
        Path::new("/proc/driver/nvidia/version").exists()
    } else {
        false
    }
}

fn has_vulkan() -> bool {
    if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        Path::new(&root)
            .join("System32")
            .join("vulkan-1.dll")
            .exists()
    } else if cfg!(target_os = "linux") {
        [
            "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
            "/usr/lib/aarch64-linux-gnu/libvulkan.so.1",
            "/usr/lib64/libvulkan.so.1",
            "/usr/lib/libvulkan.so.1",
            "/lib/x86_64-linux-gnu/libvulkan.so.1",
        ]
        .iter()
        .any(|p| Path::new(p).exists())
    } else {
        false
    }
}

#[cfg(target_os = "linux")]
fn total_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(target_os = "macos")]
fn total_memory() -> Option<u64> {
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    let name = c"hw.memsize";
    // SAFETY: `value` and `size` are valid for writes of `size` bytes, and
    // `name` is a NUL-terminated string.
    let ok = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    ok.then_some(value)
}

#[cfg(windows)]
fn total_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is a plain C struct; all-zero is valid, and
    // dwLength is set as the API requires.
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut status) != 0).then_some(status.ullTotalPhys)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn total_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(os: Os, arch: Arch, nvidia: bool, vulkan: bool) -> System {
        System {
            os,
            arch,
            nvidia,
            vulkan,
            memory_bytes: Some(16 * GIB),
        }
    }

    #[test]
    fn chooses_the_package_for_the_graphics_card() {
        assert_eq!(
            system(Os::Windows, Arch::X64, true, true).auto_backend(),
            Some(Backend::Cuda)
        );
        assert_eq!(
            system(Os::Windows, Arch::X64, false, true).auto_backend(),
            Some(Backend::Vulkan)
        );
        assert_eq!(
            system(Os::Linux, Arch::X64, false, false).auto_backend(),
            Some(Backend::Cpu)
        );
        assert_eq!(
            system(Os::Linux, Arch::X64, true, true).auto_backend(),
            Some(Backend::Vulkan)
        );
        assert_eq!(
            system(Os::Macos, Arch::Arm64, false, false).auto_backend(),
            Some(Backend::Metal)
        );
        assert_eq!(
            system(Os::Other, Arch::X64, false, false).auto_backend(),
            None
        );
        assert_eq!(
            system(Os::Linux, Arch::X64, false, false).default_context(),
            16_384
        );
        let big = System {
            memory_bytes: Some(32 * GIB),
            ..system(Os::Linux, Arch::X64, false, false)
        };
        assert_eq!(big.default_context(), 32_768);
    }

    #[test]
    fn finds_the_release_files() {
        let names: Vec<String> = [
            "llama-b9000-bin-win-cpu-x64.zip",
            "llama-b9000-bin-win-vulkan-x64.zip",
            "llama-b9000-bin-win-cuda-12.4-x64.zip",
            "llama-b9000-bin-win-cuda-13.4-x64.zip",
            "cudart-llama-bin-win-cuda-12.4-x64.zip",
            "cudart-llama-bin-win-cuda-13.4-x64.zip",
            "llama-b9000-bin-ubuntu-vulkan-x64.tar.gz",
            "llama-b9000-bin-macos-arm64.tar.gz",
        ]
        .map(String::from)
        .to_vec();
        let win = system(Os::Windows, Arch::X64, true, true);
        let cuda = win.asset_patterns(Backend::Cuda, "b9000").unwrap();
        assert_eq!(
            pick_assets(&cuda, &names).unwrap(),
            vec![
                "llama-b9000-bin-win-cuda-12.4-x64.zip",
                "cudart-llama-bin-win-cuda-12.4-x64.zip"
            ]
        );
        let vulkan = win.asset_patterns(Backend::Vulkan, "b9000").unwrap();
        assert_eq!(
            pick_assets(&vulkan, &names).unwrap(),
            vec!["llama-b9000-bin-win-vulkan-x64.zip"]
        );
        let mac = system(Os::Macos, Arch::Arm64, false, false);
        let metal = mac.asset_patterns(Backend::Metal, "b9000").unwrap();
        assert_eq!(
            pick_assets(&metal, &names).unwrap(),
            vec!["llama-b9000-bin-macos-arm64.tar.gz"]
        );
        let linux = system(Os::Linux, Arch::X64, false, false);
        let cpu = linux.asset_patterns(Backend::Cpu, "b9000").unwrap();
        assert!(pick_assets(&cpu, &names)
            .unwrap_err()
            .contains("ubuntu-x64"));
    }

    #[test]
    fn detects_this_computer() {
        let here = System::detect();
        assert!(here.memory_bytes.is_some_and(|m| m > 0) || cfg!(not(any(unix, windows))));
    }
}
