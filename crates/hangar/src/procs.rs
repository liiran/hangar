//! Process accounting and containment.
//!
//! * [`snapshot`] lists every process with its parent, private memory, working
//!   set and CPU time, so we can price a whole process tree.
//! * [`Job`] wraps a Windows job object: kill a tree as one unit, make sure
//!   nothing outlives its owner, and read exact CPU totals that include
//!   processes that have already exited.
//!
//! On other platforms the same API is backed by `sysinfo` and process groups.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// Private (committed) bytes: memory only this process uses.
    pub private_bytes: u64,
    /// Working set: resident pages, including shared ones.
    pub working_set: u64,
    /// User + kernel CPU time so far.
    pub cpu_ms: u64,
    /// Creation time in ms (platform epoch); guards against pid reuse.
    pub created_ms: u64,
}

/// All processes on the machine.
pub fn snapshot() -> Vec<ProcInfo> {
    imp::snapshot()
}

/// `root` and every process descended from it.
pub fn tree<'a>(snap: &'a [ProcInfo], root: u32) -> Vec<&'a ProcInfo> {
    trees(snap, &[root])
}

/// The union of several trees, each process counted once.
pub fn trees<'a>(snap: &'a [ProcInfo], roots: &[u32]) -> Vec<&'a ProcInfo> {
    let by_pid: HashMap<u32, &ProcInfo> = snap.iter().map(|p| (p.pid, p)).collect();
    let mut children: HashMap<u32, Vec<&ProcInfo>> = HashMap::new();
    for p in snap {
        if p.pid == p.ppid {
            continue;
        }
        // A child must be younger than its parent; otherwise the parent pid
        // was reused and the "child" is an orphan of an older process.
        if let Some(parent) = by_pid.get(&p.ppid) {
            if parent.created_ms > p.created_ms {
                continue;
            }
        }
        children.entry(p.ppid).or_default().push(p);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<&ProcInfo> = roots.iter().filter_map(|r| by_pid.get(r).copied()).collect();
    while let Some(p) = stack.pop() {
        if !seen.insert(p.pid) {
            continue;
        }
        out.push(p);
        if let Some(kids) = children.get(&p.pid) {
            stack.extend(kids.iter().copied());
        }
    }
    out
}

/// Sum of memory and CPU for a set of processes.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub processes: usize,
    pub private_bytes: u64,
    pub working_set: u64,
    pub cpu_ms: u64,
}

impl Usage {
    pub fn of<'a>(procs: impl IntoIterator<Item = &'a ProcInfo>) -> Usage {
        let mut u = Usage::default();
        for p in procs {
            u.processes += 1;
            u.private_bytes += p.private_bytes;
            u.working_set += p.working_set;
            u.cpu_ms += p.cpu_ms;
        }
        u
    }
}

/// Total and available physical memory in bytes.
pub fn system_memory() -> (u64, u64) {
    imp::system_memory()
}

/// Kills `pid` and all of its descendants.
pub fn kill_tree(pid: u32) {
    let snap = snapshot();
    let mut victims: Vec<u32> = tree(&snap, pid).iter().map(|p| p.pid).collect();
    // Children first so nothing respawns from a dying parent.
    victims.reverse();
    for v in victims {
        imp::kill(v);
    }
}

pub fn is_alive(pid: u32) -> bool {
    imp::is_alive(pid)
}

pub use imp::Job;

#[cfg(windows)]
mod imp {
    use super::ProcInfo;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
        PROCESS_TERMINATE,
    };

    fn filetime_ms(ft: &FILETIME) -> u64 {
        ((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)) / 10_000
    }

    pub fn snapshot() -> Vec<ProcInfo> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                    let mut info = ProcInfo {
                        pid: entry.th32ProcessID,
                        ppid: entry.th32ParentProcessID,
                        name,
                        private_bytes: 0,
                        working_set: 0,
                        cpu_ms: 0,
                        created_ms: 0,
                    };
                    fill(&mut info);
                    out.push(info);
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }

    fn fill(info: &mut ProcInfo) {
        if info.pid == 0 {
            return;
        }
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, info.pid);
            if h.is_null() {
                return;
            }
            let mut mem: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            if GetProcessMemoryInfo(h, &mut mem as *mut _ as *mut PROCESS_MEMORY_COUNTERS, mem.cb) != 0 {
                info.private_bytes = mem.PrivateUsage as u64;
                info.working_set = mem.WorkingSetSize as u64;
            }
            let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = std::mem::zeroed();
            if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0 {
                info.cpu_ms = filetime_ms(&k) + filetime_ms(&u);
                info.created_ms = filetime_ms(&c);
            }
            CloseHandle(h);
        }
    }

    pub fn system_memory() -> (u64, u64) {
        unsafe {
            let mut st: MEMORYSTATUSEX = std::mem::zeroed();
            st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut st) != 0 { (st.ullTotalPhys, st.ullAvailPhys) } else { (0, 0) }
        }
    }

    pub fn kill(pid: u32) {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                TerminateProcess(h, 1);
                CloseHandle(h);
            }
        }
    }

    pub fn is_alive(pid: u32) -> bool {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            ok && code == STILL_ACTIVE as u32
        }
    }

    /// A Windows job object.
    pub struct Job {
        handle: HANDLE,
    }

    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// A job whose processes all die when the last handle to it closes.
        pub fn new_kill_on_close() -> std::io::Result<Job> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let job = Job { handle };
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        pub fn assign_pid(&self, pid: u32) -> std::io::Result<()> {
            unsafe {
                let h = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if h.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let ok = AssignProcessToJobObject(self.handle, h);
                CloseHandle(h);
                if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
            }
        }

        pub fn assign_self(&self) -> std::io::Result<()> {
            unsafe {
                if AssignProcessToJobObject(self.handle, GetCurrentProcess()) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        }

        /// Total CPU time (user + kernel) of every process that ever ran in the job.
        pub fn cpu_ms(&self) -> u64 {
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                if QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return 0;
                }
                ((info.TotalUserTime + info.TotalKernelTime) / 10_000) as u64
            }
        }

        /// Pids of processes currently in the job.
        pub fn pids(&self) -> Vec<u32> {
            const CAP: usize = 4096;
            #[repr(C)]
            struct List {
                assigned: u32,
                in_list: u32,
                ids: [usize; CAP],
            }
            unsafe {
                let mut list: Box<List> = Box::new(std::mem::zeroed());
                if QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicProcessIdList,
                    &mut *list as *mut List as *mut core::ffi::c_void,
                    std::mem::size_of::<List>() as u32,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Vec::new();
                }
                list.ids[..(list.in_list as usize).min(CAP)].iter().map(|&p| p as u32).collect()
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.handle, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::ProcInfo;
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    pub fn snapshot() -> Vec<ProcInfo> {
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::everything());
        sys.processes()
            .values()
            .map(|p| ProcInfo {
                pid: p.pid().as_u32(),
                ppid: p.parent().map(|x| x.as_u32()).unwrap_or(0),
                name: p.name().to_string_lossy().into_owned(),
                private_bytes: p.memory(),
                working_set: p.memory(),
                cpu_ms: p.accumulated_cpu_time(),
                created_ms: p.start_time() * 1000,
            })
            .collect()
    }

    pub fn system_memory() -> (u64, u64) {
        let mut sys = System::new();
        sys.refresh_memory();
        (sys.total_memory(), sys.available_memory())
    }

    pub fn kill(pid: u32) {
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }

    pub fn is_alive(pid: u32) -> bool {
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
        sys.process(Pid::from_u32(pid)).is_some()
    }

    /// Process-group based stand-in for a job: tracks a set of root pids.
    pub struct Job {
        roots: std::sync::Mutex<Vec<u32>>,
    }

    impl Job {
        pub fn new_kill_on_close() -> std::io::Result<Job> {
            Ok(Job { roots: std::sync::Mutex::new(Vec::new()) })
        }
        pub fn assign_pid(&self, pid: u32) -> std::io::Result<()> {
            self.roots.lock().unwrap().push(pid);
            Ok(())
        }
        pub fn assign_self(&self) -> std::io::Result<()> {
            Ok(())
        }
        pub fn cpu_ms(&self) -> u64 {
            let snap = snapshot();
            let roots = self.roots.lock().unwrap().clone();
            super::trees(&snap, &roots).iter().map(|p| p.cpu_ms).sum()
        }
        pub fn pids(&self) -> Vec<u32> {
            let snap = snapshot();
            let roots = self.roots.lock().unwrap().clone();
            super::trees(&snap, &roots).iter().map(|p| p.pid).collect()
        }
        pub fn terminate(&self) {
            for pid in self.pids() {
                kill(pid);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            self.terminate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, created: u64) -> ProcInfo {
        ProcInfo { pid, ppid, name: format!("p{pid}"), private_bytes: 10, working_set: 20, cpu_ms: 1, created_ms: created }
    }

    #[test]
    fn tree_follows_children_and_ignores_reused_pids() {
        // 1 -> 2 -> 3 ; 4 claims parent 1 but is older than 1 (pid reuse).
        let snap = vec![p(1, 0, 100), p(2, 1, 110), p(3, 2, 120), p(4, 1, 50), p(5, 9, 10)];
        let mut got: Vec<u32> = tree(&snap, 1).iter().map(|p| p.pid).collect();
        got.sort();
        assert_eq!(got, vec![1, 2, 3]);
        assert_eq!(Usage::of(tree(&snap, 2)).processes, 2);
    }

    #[test]
    fn trees_deduplicate() {
        let snap = vec![p(1, 0, 1), p(2, 1, 2), p(3, 2, 3)];
        assert_eq!(trees(&snap, &[1, 2, 3]).len(), 3);
    }

    #[test]
    fn snapshot_sees_this_process() {
        let snap = snapshot();
        let me = snap.iter().find(|p| p.pid == std::process::id()).expect("own process in snapshot");
        assert!(me.private_bytes > 0, "private bytes measured");
        let (total, avail) = system_memory();
        assert!(total > 0 && avail > 0 && avail <= total);
    }
}
