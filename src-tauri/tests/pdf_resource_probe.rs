#![cfg(windows)]

use std::{
    io::{Read, Write},
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::{JobObjects::*, Memory::*, Threading::CREATE_NO_WINDOW};

struct Report {
    elapsed: Duration,
    peak_committed_bytes: usize,
    timed_out: bool,
    success: bool,
}

fn run_probe(action: &str, memory: usize, timeout: Duration) -> Report {
    // The child waits on stdin before parsing or allocating; attach the job before releasing it.
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!handle.is_null(), "{}", std::io::Error::last_os_error());
    let job = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    limits.ProcessMemoryLimit = memory;
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of_val(&limits) as u32,
        )
    };
    assert_ne!(configured, 0, "{}", std::io::Error::last_os_error());
    let started = Instant::now();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "worker_entry", "--ignored", "--nocapture"])
        .env("FCS_RESOURCE_PROBE", action)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
        let error = std::io::Error::last_os_error();
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("Cannot attach resource job: {error}");
    }
    child.stdin.take().unwrap().write_all(b"1").unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            break None;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut accounting = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    let queried = unsafe {
        QueryInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&mut accounting as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of_val(&accounting) as u32,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(queried, 0, "{}", std::io::Error::last_os_error());
    drop(job);
    if status.is_none() {
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "child survived closing its job");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Report {
        elapsed: started.elapsed(),
        peak_committed_bytes: accounting.PeakProcessMemoryUsed,
        timed_out: status.is_none(),
        success: status.is_some_and(|exit| exit.success()),
    }
}

#[test]
#[ignore = "internal child entry; invoked only by the parent probe"]
fn worker_entry() {
    let Ok(action) = std::env::var("FCS_RESOURCE_PROBE") else {
        return;
    };
    std::io::stdin().read_exact(&mut [0u8; 1]).unwrap();
    match action.as_str() {
        "memory" => {
            let allocation = unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    128 * 1024 * 1024,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            if !allocation.is_null() {
                unsafe {
                    VirtualFree(allocation, 0, MEM_RELEASE);
                }
            }
            assert!(
                allocation.is_null(),
                "OS accepted allocation above the job memory limit"
            );
        }
        "wait" => std::thread::sleep(Duration::from_secs(30)),
        "pdf" => {
            let document = file_content_search::extract::extract_bytes(
                include_bytes!("fixtures/chinese.pdf"),
                "pdf",
                Default::default(),
            )
            .unwrap();
            assert!(document.text.contains("合同签署 ABC 😀"));
            assert_eq!(document.locations.len(), 2);
        }
        "empty" => {}
        _ => panic!("unknown probe"),
    }
}

#[test]
fn os_denies_over_budget_allocation() {
    let result = run_probe("memory", 64 * 1024 * 1024, Duration::from_secs(10));
    assert!(result.success && !result.timed_out);
    assert!(result.peak_committed_bytes <= 64 * 1024 * 1024);
}

#[test]
fn closing_job_stops_timed_out_child() {
    let result = run_probe("wait", 64 * 1024 * 1024, Duration::from_millis(150));
    assert!(result.timed_out);
    assert!(result.elapsed < Duration::from_secs(3));
}

#[test]
fn measure_bounded_pdf_child() {
    for action in ["empty", "pdf"] {
        let mut durations = Vec::new();
        let mut peak = 0;
        for _ in 0..20 {
            let result = run_probe(action, 512 * 1024 * 1024, Duration::from_secs(10));
            assert!(result.success && !result.timed_out);
            durations.push(result.elapsed.as_secs_f64() * 1000.0);
            peak = peak.max(result.peak_committed_bytes);
        }
        durations.sort_by(f64::total_cmp);
        println!(
            "{action}: process+handshake+work+exit P50={:.2}ms P95={:.2}ms peak_commit={:.2}MiB",
            durations[9],
            durations[18],
            peak as f64 / 1048576.0
        );
    }
}
