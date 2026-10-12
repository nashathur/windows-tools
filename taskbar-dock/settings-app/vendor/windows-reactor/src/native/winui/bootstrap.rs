use crate::native::winui::bindings::*;
use windows_core::*;

const PACKAGE_DEPENDENCY_LIFETIME_KIND_PROCESS: i32 = 0;
static BOOTSTRAPPED: std::sync::Mutex<Option<RuntimePackage>> = std::sync::Mutex::new(None);

#[derive(Clone, Debug)]
pub struct RuntimePackage {
    pub family: String,
    pub full_name: String,
    pub path: String,
}

/// Adds the best available Windows App Runtime package to the process package graph.
pub fn bootstrap() -> Result<RuntimePackage> {
    let mut bootstrapped = BOOTSTRAPPED.lock().unwrap();
    if let Some(package) = bootstrapped.clone() { return Ok(package); }
    let package = unsafe { bootstrap_inner()? };
    *bootstrapped = Some(package.clone());
    Ok(package)
}

unsafe fn bootstrap_inner() -> Result<RuntimePackage> {
    for family in ["Microsoft.WindowsAppRuntime.CBS.2_8wekyb3d8bbwe", "Microsoft.WindowsAppRuntime.2_8wekyb3d8bbwe"] {
        let family_wide: Vec<u16> = family.encode_utf16().chain(Some(0)).collect();
        let mut dependency_id = PWSTR::null();
        let hr = unsafe { TryCreatePackageDependency(
            std::ptr::null_mut(), PCWSTR(family_wide.as_ptr()),
            PACKAGE_VERSION { Anonymous: PACKAGE_VERSION_0 { Version: WINDOWSAPPSDK_RUNTIME_VERSION_UINT64 } },
            process_architecture().flags | PackageDependencyProcessorArchitectures_Neutral,
            PACKAGE_DEPENDENCY_LIFETIME_KIND_PROCESS, PCWSTR::null(), 0, &mut dependency_id,
        ) };
        if hr == STATEREPOSITORY_E_DEPENDENCY_NOT_RESOLVED { continue; }
        hr.ok()?;
        let mut handle = std::ptr::null_mut();
        let mut full_name = PWSTR::null();
        let added = unsafe { AddPackageDependency(PCWSTR(dependency_id.0), 0, 0, &mut handle, &mut full_name) };
        unsafe { let _ = HeapFree(GetProcessHeap(), 0, dependency_id.0 as *mut _); }
        if added == STATEREPOSITORY_E_DEPENDENCY_NOT_RESOLVED { continue; }
        added.ok()?;
        let full_name_string = unsafe { full_name.to_string()? };
        let mut length = 0u32;
        let _ = unsafe { GetPackagePathByFullName(PCWSTR(full_name.0), &mut length, PWSTR::null()) };
        if length == 0 {
            unsafe { let _ = HeapFree(GetProcessHeap(), 0, full_name.0 as *mut _); }
            return Err(Error::new(HRESULT(0x80070002_u32 as i32), "GetPackagePathByFullName failed"));
        }
        let mut path = vec![0u16; length as usize];
        let path_result = unsafe { GetPackagePathByFullName(PCWSTR(full_name.0), &mut length, PWSTR(path.as_mut_ptr())) };
        unsafe { let _ = HeapFree(GetProcessHeap(), 0, full_name.0 as *mut _); }
        if path_result != 0 {
            return Err(Error::new(HRESULT((0x8007_0000u32 | (path_result as u32 & 0xFFFF)) as i32), "GetPackagePathByFullName failed"));
        }
        let end = (length.saturating_sub(1) as usize).min(path.len());
        let package_path = String::from_utf16_lossy(&path[..end]);
        return Ok(RuntimePackage { family: family.to_string(), full_name: full_name_string, path: package_path });
    }
    Err(Error::new(STATEREPOSITORY_E_DEPENDENCY_NOT_RESOLVED, "no compatible Windows App Runtime package is installed"))
}

windows_core::link!("kernel32.dll" "system" fn GetPackagePathByFullName(packagefullname: PCWSTR, packagepathlength: *mut u32, packagepath: PWSTR) -> i32);

#[derive(Clone, Copy)]
struct ProcessArchitecture { flags: PackageDependencyProcessorArchitectures }
fn process_architecture() -> ProcessArchitecture {
    let flags = match std::env::consts::ARCH {
        "x86_64" => PackageDependencyProcessorArchitectures_X64,
        "x86" => PackageDependencyProcessorArchitectures_X86,
        "aarch64" => PackageDependencyProcessorArchitectures_Arm64,
        "arm" => PackageDependencyProcessorArchitectures_Arm,
        _ => PackageDependencyProcessorArchitectures_None,
    };
    ProcessArchitecture { flags }
}
