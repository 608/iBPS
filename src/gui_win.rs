use std::ffi::{OsStr, OsString};
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::mpsc;

use windows_sys::Win32::Foundation::{
    ERROR_SUCCESS, HINSTANCE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, GetMonitorInfoW, HFONT, LOGFONTW,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, UpdateWindow,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_CHAR, GetFileType};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE,
    STD_OUTPUT_HANDLE, SetStdHandle,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegSetValueExW,
};
use windows_sys::Win32::System::SystemServices::SS_RIGHT;
use windows_sys::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST,
    OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE, GetDpiForSystem, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, IsWindowEnabled, SetFocus, VK_ESCAPE, VK_RETURN,
};
use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, BM_SETCHECK, BN_CLICKED, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON,
    BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWNLIST,
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOHSCROLL, GWLP_USERDATA, GetCursorPos, GetDlgItem, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    IDC_ARROW, IDYES, IMAGE_ICON, IsDialogMessageW, IsWindowVisible, LR_SHARED, LoadCursorW,
    LoadImageW, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO,
    MSG, MessageBoxW, NONCLIENTMETRICSW, PostMessageW, PostQuitMessage, RegisterClassExW,
    SM_CXICON, SM_CXSCREEN, SM_CXSMICON, SM_CYICON, SM_CYSCREEN, SM_CYSMICON,
    SPI_GETNONCLIENTMETRICS, SPI_GETWORKAREA, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetProcessDPIAware, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW, TranslateMessage, WM_APP,
    WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_NCCREATE, WM_NCDESTROY, WM_SETFONT,
    WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_GROUP, WS_MINIMIZEBOX, WS_OVERLAPPED,
    WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::files::{apply_files, create_files, default_apply_path};
use crate::patch::Format;
use crate::settings_win::{self, GuiSettings};

const ID_MODE_APPLY: usize = 1001;
const ID_MODE_CREATE: usize = 1002;
const ID_FORMAT: usize = 1003;
const ID_FORMAT_LABEL: usize = 1004;
const ID_PATCH_LABEL: usize = 1010;
const ID_PATCH_EDIT: usize = 1011;
const ID_PATCH_BROWSE: usize = 1012;
const ID_SOURCE_LABEL: usize = 1020;
const ID_SOURCE_EDIT: usize = 1021;
const ID_SOURCE_BROWSE: usize = 1022;
const ID_TARGET_LABEL: usize = 1030;
const ID_TARGET_EDIT: usize = 1031;
const ID_TARGET_BROWSE: usize = 1032;
const ID_ASSOCIATE: usize = 1040;
const ID_EXECUTE: usize = 1041;
const WM_WORK_DONE: u32 = WM_APP + 1;
const FILE_BUFFER_LEN: usize = 32_768;

const PATCH_FILTER: &str =
    "パッチファイル (*.ips;*.ups;*.bps)\0*.ips;*.ups;*.bps\0すべてのファイル (*.*)\0*.*\0\0";
const ROM_FILTER: &str = "ROMファイル (*.gba;*.gbc;*.gb;*.sfc;*.smc;*.nes;*.bin;*.rom)\0*.gba;*.gbc;*.gb;*.sfc;*.smc;*.nes;*.bin;*.rom\0すべてのファイル (*.*)\0*.*\0\0";
const ALL_FILTER: &str = "すべてのファイル (*.*)\0*.*\0\0";

#[derive(Default)]
struct Controls {
    apply: HWND,
    create: HWND,
    format: HWND,
    patch_edit: HWND,
    source_edit: HWND,
    target_label: HWND,
    target_edit: HWND,
    execute: HWND,
}

struct UiState {
    settings: GuiSettings,
    controls: Controls,
    font: HFONT,
    dpi: u32,
}

pub fn attach_console() {
    unsafe {
        let stdout = redirected_handle(STD_OUTPUT_HANDLE);
        let stderr = redirected_handle(STD_ERROR_HANDLE);
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        if let Some(handle) = stdout {
            SetStdHandle(STD_OUTPUT_HANDLE, handle);
        }
        if let Some(handle) = stderr {
            SetStdHandle(STD_ERROR_HANDLE, handle);
        }
    }
}

unsafe fn redirected_handle(kind: u32) -> Option<*mut core::ffi::c_void> {
    let handle = unsafe { GetStdHandle(kind) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut mode = 0;
    if unsafe { GetFileType(handle) } != FILE_TYPE_CHAR
        || unsafe { GetConsoleMode(handle, &mut mode) } == 0
    {
        Some(handle)
    } else {
        None
    }
}

pub fn run(patch: Option<PathBuf>) -> i32 {
    // Use the same coordinate system for the monitor work area and window placement.
    unsafe { SetProcessDPIAware() };
    let mut settings = match settings_win::load() {
        Ok(settings) => settings,
        Err(error) => {
            message_box(null_mut(), &error, MB_ICONERROR);
            GuiSettings::default()
        }
    };
    let associated = patch.is_some();
    if let Some(patch) = patch {
        prepare_associated(&mut settings, patch);
    }
    unsafe { run_main_window(settings, associated) }
}

fn prepare_associated(settings: &mut GuiSettings, patch: PathBuf) {
    settings.create = false;
    settings.apply_patch = normalized_path(patch);
}

fn select_associated_source(settings: &mut GuiSettings, source: PathBuf) -> PathBuf {
    settings.source = normalized_path(source);
    default_apply_path(&settings.apply_patch, &settings.source)
}

fn centered_origin(work: &RECT, width: i32, height: i32) -> (i32, i32) {
    (
        work.left + ((work.right - work.left - width).max(0) / 2),
        work.top + ((work.bottom - work.top - height).max(0) / 2),
    )
}

unsafe fn launch_work_area() -> RECT {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) };
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        return info.rcWork;
    }
    let mut work = RECT {
        left: 0,
        top: 0,
        right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
        bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
    };
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0) };
    work
}

unsafe fn center_created_window(hwnd: HWND) {
    // Align the created window frame with the work area in physical coordinates across DPI settings.
    let previous = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE) };
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
        let work = unsafe { launch_work_area() };
        let (x, y) = centered_origin(&work, rect.right - rect.left, rect.bottom - rect.top);
        unsafe {
            SetWindowPos(
                hwnd,
                null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }
    if !previous.is_null() {
        unsafe { SetThreadDpiAwarenessContext(previous) };
    }
}

unsafe fn run_main_window(settings: GuiSettings, associated: bool) -> i32 {
    let instance = unsafe { GetModuleHandleW(null()) };
    let load_icon = |width, height| unsafe {
        LoadImageW(
            instance,
            std::ptr::without_provenance(1), // MAKEINTRESOURCEW(1)
            IMAGE_ICON,
            GetSystemMetrics(width),
            GetSystemMetrics(height),
            LR_SHARED,
        )
    };
    let icon = load_icon(SM_CXICON, SM_CYICON);
    let small_icon = load_icon(SM_CXSMICON, SM_CYSMICON);
    let class_name = wide("ibps.main.window");
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: icon,
        hIconSm: small_icon,
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        hbrBackground: (COLOR_BTNFACE + 1) as usize as _,
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        message_box(
            null_mut(),
            "ウィンドウを初期化できませんでした。",
            MB_ICONERROR,
        );
        return 1;
    }
    let processing_class_name = wide("ibps.processing.window");
    let processing_class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        hIcon: icon,
        hIconSm: small_icon,
        lpfnWndProc: Some(processing_window_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        hbrBackground: (COLOR_BTNFACE + 1) as usize as _,
        lpszClassName: processing_class_name.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&processing_class) } == 0 {
        message_box(
            null_mut(),
            "ウィンドウを初期化できませんでした。",
            MB_ICONERROR,
        );
        return 1;
    }

    let dpi = unsafe { GetDpiForSystem() }.max(96);
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale(460, dpi),
        bottom: scale(160, dpi),
    };
    unsafe { AdjustWindowRectEx(&mut rect, style, 0, 0) };
    let (x, y) = centered_origin(
        &unsafe { launch_work_area() },
        rect.right - rect.left,
        rect.bottom - rect.top,
    );
    let mut state = Box::new(UiState {
        settings,
        controls: Controls::default(),
        font: null_mut(),
        dpi,
    });
    let state_ptr: *mut UiState = &mut *state;
    let title = wide("iBPS");
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            style,
            x,
            y,
            rect.right - rect.left,
            rect.bottom - rect.top,
            null_mut(),
            null_mut(),
            instance,
            state_ptr.cast(),
        )
    };
    if hwnd.is_null() {
        message_box(
            null_mut(),
            "ウィンドウを作成できませんでした。",
            MB_ICONERROR,
        );
        return 1;
    }
    unsafe { center_created_window(hwnd) };
    if associated {
        // Use this window only as the owner of file dialogs and the progress display.
        let mut settings = unsafe { snapshot(hwnd) }.expect("Initialized window state");
        let result = unsafe { apply_associated(hwnd, &mut settings) };
        if let Err(error) = settings_win::save(&settings) {
            message_box(hwnd, &error, MB_ICONERROR);
        }
        let code = match result {
            Ok(()) => 0,
            Err(error) => {
                message_box(hwnd, &error, MB_ICONERROR);
                1
            }
        };
        unsafe { DestroyWindow(hwnd) };
        return code;
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetFocus(GetDlgItem(
            hwnd,
            if (*state_ptr).settings.create {
                ID_MODE_CREATE
            } else {
                ID_MODE_APPLY
            } as i32,
        ));
    }

    let mut msg = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if status == 0 {
            unsafe { DestroyWindow(hwnd) };
            return msg.wParam as i32;
        }
        if status == -1 {
            unsafe { DestroyWindow(hwnd) };
            return 1;
        }
        unsafe {
            if msg.message == WM_KEYDOWN && msg.wParam == VK_ESCAPE as usize {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            } else if msg.message == WM_KEYDOWN
                && msg.wParam == VK_RETURN as usize
                && focus_is_path_edit(hwnd)
            {
                PostMessageW(hwnd, WM_COMMAND, ID_EXECUTE, 0);
            } else if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

unsafe fn apply_associated(owner: HWND, settings: &mut GuiSettings) -> Result<(), String> {
    crate::patch::info(&crate::files::read(&settings.apply_patch)?)?;
    let initial = (!settings.source.as_os_str().is_empty()).then_some(settings.source.as_path());
    let Some(source) = (unsafe {
        file_dialog(
            owner,
            "適用先のファイルを選択",
            ROM_FILTER,
            initial,
            false,
            None,
        )
    })?
    else {
        return Ok(());
    };
    // Derive the default name from the current patch; preserve the saved output path on cancellation.
    let mut output = select_associated_source(settings, source);
    validate_input(&settings.source, "適用先ファイル")?;
    loop {
        let Some(selected) = (unsafe {
            file_dialog(
                owner,
                "保存先を選択",
                ALL_FILTER,
                Some(&output),
                true,
                settings.source.extension().and_then(OsStr::to_str),
            )
        })?
        else {
            return Ok(());
        };
        output = normalized_path(selected);
        validate_output(&output)?;
        if confirm_existing(owner, &output) {
            break;
        }
    }
    settings.output = output.clone();
    let patch = settings.apply_patch.clone();
    let source = settings.source.clone();
    let warnings =
        unsafe { run_worker(owner, move || apply_files(&patch, &source, Some(&output))) }?;
    show_result(owner, "パッチを適用しました。", &settings.output, &warnings);
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Ignore execution and close notifications queued during modal processing.
    if matches!(message, WM_COMMAND | WM_CLOSE) && unsafe { IsWindowEnabled(hwnd) } == 0 {
        return 0;
    }
    match message {
        WM_NCCREATE => {
            let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_CREATE => {
            if unsafe { create_controls(hwnd) } {
                0
            } else {
                -1
            }
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            let notification = ((wparam >> 16) & 0xffff) as u32;
            if notification == BN_CLICKED {
                match id {
                    ID_MODE_APPLY => unsafe { switch_mode(hwnd, false) },
                    ID_MODE_CREATE => unsafe { switch_mode(hwnd, true) },
                    ID_PATCH_BROWSE => unsafe { browse(hwnd, ID_PATCH_EDIT) },
                    ID_SOURCE_BROWSE => unsafe { browse(hwnd, ID_SOURCE_EDIT) },
                    ID_TARGET_BROWSE => unsafe { browse(hwnd, ID_TARGET_EDIT) },
                    ID_ASSOCIATE => unsafe { associate(hwnd) },
                    ID_EXECUTE => unsafe { execute(hwnd) },
                    _ => {}
                }
            } else if id == ID_FORMAT && notification == CBN_SELCHANGE {
                // The selected format is collected on mode switch, execution, or close.
            }
            0
        }
        WM_CLOSE => {
            unsafe { collect_current(hwnd) };
            if let Err(error) = unsafe { save_snapshot(hwnd) } {
                message_box(hwnd, &error, MB_ICONERROR);
            }
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_NCDESTROY => {
            let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut UiState };
            let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            if !pointer.is_null() {
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
                let font = unsafe { (*pointer).font };
                if !font.is_null() {
                    unsafe {
                        DeleteObject(font);
                        (*pointer).font = null_mut();
                    }
                }
            }
            result
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe fn create_controls(hwnd: HWND) -> bool {
    let pointer = unsafe { state_ptr(hwnd) };
    if pointer.is_null() {
        return false;
    }
    let dpi = unsafe { (*pointer).dpi };
    let instance = unsafe { GetModuleHandleW(null()) };
    let button = wide("BUTTON");
    let static_class = wide("STATIC");
    let edit = wide("EDIT");
    let combo = wide("COMBOBOX");

    let apply = unsafe {
        control(
            hwnd,
            instance,
            0,
            &button,
            "パッチ適用",
            WS_GROUP | WS_TABSTOP | BS_AUTORADIOBUTTON as u32,
            10,
            7,
            90,
            21,
            ID_MODE_APPLY,
            dpi,
        )
    };
    let create = unsafe {
        control(
            hwnd,
            instance,
            0,
            &button,
            "パッチ作成",
            BS_AUTORADIOBUTTON as u32,
            110,
            7,
            90,
            21,
            ID_MODE_CREATE,
            dpi,
        )
    };
    unsafe {
        control(
            hwnd,
            instance,
            0,
            &static_class,
            "フォーマット",
            SS_RIGHT,
            225,
            10,
            100,
            18,
            ID_FORMAT_LABEL,
            dpi,
        )
    };
    let format = unsafe {
        control(
            hwnd,
            instance,
            0,
            &combo,
            "",
            WS_GROUP | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            333,
            7,
            117,
            120,
            ID_FORMAT,
            dpi,
        )
    };

    unsafe {
        control(
            hwnd,
            instance,
            0,
            &static_class,
            "パッチファイル",
            0,
            10,
            37,
            106,
            20,
            ID_PATCH_LABEL,
            dpi,
        )
    };
    let patch_edit = unsafe {
        control(
            hwnd,
            instance,
            WS_EX_CLIENTEDGE,
            &edit,
            "",
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            120,
            34,
            268,
            22,
            ID_PATCH_EDIT,
            dpi,
        )
    };
    unsafe {
        control(
            hwnd,
            instance,
            0,
            &button,
            "参照...",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            393,
            33,
            57,
            24,
            ID_PATCH_BROWSE,
            dpi,
        )
    };

    unsafe {
        control(
            hwnd,
            instance,
            0,
            &static_class,
            "適用先ファイル",
            0,
            10,
            64,
            106,
            20,
            ID_SOURCE_LABEL,
            dpi,
        )
    };
    let source_edit = unsafe {
        control(
            hwnd,
            instance,
            WS_EX_CLIENTEDGE,
            &edit,
            "",
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            120,
            61,
            268,
            22,
            ID_SOURCE_EDIT,
            dpi,
        )
    };
    unsafe {
        control(
            hwnd,
            instance,
            0,
            &button,
            "参照...",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            393,
            60,
            57,
            24,
            ID_SOURCE_BROWSE,
            dpi,
        )
    };

    let target_label = unsafe {
        control(
            hwnd,
            instance,
            0,
            &static_class,
            "変更後ファイル",
            0,
            10,
            91,
            108,
            20,
            ID_TARGET_LABEL,
            dpi,
        )
    };
    let target_edit = unsafe {
        control(
            hwnd,
            instance,
            WS_EX_CLIENTEDGE,
            &edit,
            "",
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            120,
            88,
            268,
            22,
            ID_TARGET_EDIT,
            dpi,
        )
    };
    unsafe {
        control(
            hwnd,
            instance,
            0,
            &button,
            "参照...",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            393,
            87,
            57,
            24,
            ID_TARGET_BROWSE,
            dpi,
        )
    };

    unsafe {
        control(
            hwnd,
            instance,
            0,
            &button,
            "拡張子の関連付け",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            10,
            124,
            128,
            25,
            ID_ASSOCIATE,
            dpi,
        )
    };
    let execute = unsafe {
        control(
            hwnd,
            instance,
            0,
            &button,
            "実行",
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            362,
            124,
            88,
            25,
            ID_EXECUTE,
            dpi,
        )
    };

    let required = [
        apply,
        create,
        format,
        patch_edit,
        source_edit,
        target_label,
        target_edit,
        execute,
    ];
    if required.iter().any(|handle| handle.is_null()) {
        return false;
    }
    for name in ["BPS", "IPS", "IPS32", "UPS"] {
        let name = wide(name);
        unsafe { SendMessageW(format, CB_ADDSTRING, 0, name.as_ptr() as LPARAM) };
    }
    let font = unsafe { system_font(dpi) };
    let children = [
        ID_MODE_APPLY,
        ID_MODE_CREATE,
        ID_FORMAT,
        ID_FORMAT_LABEL,
        ID_PATCH_LABEL,
        ID_PATCH_EDIT,
        ID_PATCH_BROWSE,
        ID_SOURCE_LABEL,
        ID_SOURCE_EDIT,
        ID_SOURCE_BROWSE,
        ID_TARGET_LABEL,
        ID_TARGET_EDIT,
        ID_TARGET_BROWSE,
        ID_ASSOCIATE,
        ID_EXECUTE,
    ];
    if !font.is_null() {
        for id in children {
            let child = unsafe { GetDlgItem(hwnd, id as i32) };
            if !child.is_null() {
                unsafe { SendMessageW(child, WM_SETFONT, font as WPARAM, 1) };
            }
        }
    }
    unsafe {
        (*pointer).font = font;
        (*pointer).controls = Controls {
            apply,
            create,
            format,
            patch_edit,
            source_edit,
            target_label,
            target_edit,
            execute,
        };
    }
    unsafe { display_mode(hwnd) };
    true
}

#[allow(clippy::too_many_arguments)]
unsafe fn control(
    parent: HWND,
    instance: HINSTANCE,
    ex_style: u32,
    class: &[u16],
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: usize,
    dpi: u32,
) -> HWND {
    let text = wide(text);
    unsafe {
        CreateWindowExW(
            ex_style,
            class.as_ptr(),
            text.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            scale(x, dpi),
            scale(y, dpi),
            scale(width, dpi),
            scale(height, dpi),
            parent,
            if id == 0 { null_mut() } else { id as *mut _ },
            instance,
            null(),
        )
    }
}

unsafe fn system_font(dpi: u32) -> HFONT {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    if unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            (&mut metrics as *mut NONCLIENTMETRICSW).cast(),
            0,
        )
    } != 0
    {
        return unsafe { CreateFontIndirectW(&metrics.lfMessageFont) };
    }
    let mut fallback = LOGFONTW {
        lfHeight: -scale(12, dpi),
        ..Default::default()
    };
    let face: Vec<u16> = "Segoe UI".encode_utf16().collect();
    fallback.lfFaceName[..face.len()].copy_from_slice(&face);
    unsafe { CreateFontIndirectW(&fallback) }
}

unsafe fn switch_mode(hwnd: HWND, create: bool) {
    unsafe { collect_current(hwnd) };
    let pointer = unsafe { state_ptr(hwnd) };
    if pointer.is_null() {
        return;
    }
    unsafe { (*pointer).settings.create = create };
    unsafe { display_mode(hwnd) };
    if let Err(error) = unsafe { save_snapshot(hwnd) } {
        message_box(hwnd, &error, MB_ICONERROR);
    }
}

unsafe fn display_mode(hwnd: HWND) {
    let pointer = unsafe { state_ptr(hwnd) };
    if pointer.is_null() {
        return;
    }
    let (create, format, patch, source, target, controls) = unsafe {
        let state = &*pointer;
        let patch = if state.settings.create {
            state.settings.create_patch.clone()
        } else {
            state.settings.apply_patch.clone()
        };
        let target = if state.settings.create {
            state.settings.target.clone()
        } else {
            state.settings.output.clone()
        };
        (
            state.settings.create,
            state.settings.format,
            patch,
            state.settings.source.clone(),
            target,
            (
                state.controls.apply,
                state.controls.create,
                state.controls.format,
                state.controls.patch_edit,
                state.controls.source_edit,
                state.controls.target_label,
                state.controls.target_edit,
                state.controls.execute,
            ),
        )
    };
    let source_label = wide(if create {
        "変更前ファイル"
    } else {
        "適用先ファイル"
    });
    let target_label = wide(if create {
        "変更後ファイル"
    } else {
        "保存先ファイル"
    });
    let execute_label = wide("実行");
    unsafe {
        SendMessageW(
            controls.0,
            BM_SETCHECK,
            if create { BST_UNCHECKED } else { BST_CHECKED } as WPARAM,
            0,
        );
        SendMessageW(
            controls.1,
            BM_SETCHECK,
            if create { BST_CHECKED } else { BST_UNCHECKED } as WPARAM,
            0,
        );
        EnableWindow(controls.2, create as i32);
        SendMessageW(controls.2, CB_SETCURSEL, format_index(format), 0);
        set_path_text(controls.3, &patch);
        set_path_text(controls.4, &source);
        set_path_text(controls.6, &target);
        SetWindowTextW(
            GetDlgItem(hwnd, ID_SOURCE_LABEL as i32),
            source_label.as_ptr(),
        );
        SetWindowTextW(controls.5, target_label.as_ptr());
        SetWindowTextW(controls.7, execute_label.as_ptr());
    }
    unsafe { arrange_file_rows(hwnd, create) };
}

unsafe fn arrange_file_rows(hwnd: HWND, create: bool) {
    let pointer = unsafe { state_ptr(hwnd) };
    let dpi = unsafe { (*pointer).dpi };
    let patch = [ID_PATCH_LABEL, ID_PATCH_EDIT, ID_PATCH_BROWSE];
    let source = [ID_SOURCE_LABEL, ID_SOURCE_EDIT, ID_SOURCE_BROWSE];
    let target = [ID_TARGET_LABEL, ID_TARGET_EDIT, ID_TARGET_BROWSE];
    let rows = if create {
        [source, target, patch]
    } else {
        [patch, source, target]
    };
    let mut previous = unsafe { GetDlgItem(hwnd, ID_FORMAT as i32) };
    for (row, ids) in rows.iter().enumerate() {
        for (&id, (x, offset)) in ids.iter().zip([(10, 4), (120, 1), (393, 0)]) {
            let child = unsafe { GetDlgItem(hwnd, id as i32) };
            // Reorder child windows so tab navigation follows the displayed order.
            unsafe {
                SetWindowPos(
                    child,
                    previous,
                    scale(x, dpi),
                    scale(33 + row as i32 * 27 + offset, dpi),
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            previous = child;
        }
    }
    for id in [ID_ASSOCIATE, ID_EXECUTE] {
        let child = unsafe { GetDlgItem(hwnd, id as i32) };
        unsafe {
            SetWindowPos(
                child,
                previous,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
        previous = child;
    }
}

unsafe fn collect_current(hwnd: HWND) {
    let pointer = unsafe { state_ptr(hwnd) };
    if pointer.is_null() {
        return;
    }
    let (create, patch_edit, source_edit, target_edit, format_control) = unsafe {
        let state = &*pointer;
        (
            state.settings.create,
            state.controls.patch_edit,
            state.controls.source_edit,
            state.controls.target_edit,
            state.controls.format,
        )
    };
    let patch = normalized_path(unsafe { path_text(patch_edit) });
    let source = normalized_path(unsafe { path_text(source_edit) });
    let target = normalized_path(unsafe { path_text(target_edit) });
    let selected = unsafe { SendMessageW(format_control, CB_GETCURSEL, 0, 0) };
    let state = unsafe { &mut *pointer };
    state.settings.source = source;
    state.settings.format = format_from_index(selected);
    if create {
        state.settings.create_patch = patch;
        state.settings.target = target;
    } else {
        state.settings.apply_patch = patch;
        state.settings.output = target;
    }
}

fn normalized_path(path: PathBuf) -> PathBuf {
    if path.as_os_str().is_empty() {
        path
    } else {
        std::path::absolute(&path).unwrap_or(path)
    }
}

unsafe fn browse(hwnd: HWND, edit_id: usize) {
    let pointer = unsafe { state_ptr(hwnd) };
    if pointer.is_null() {
        return;
    }
    let create = unsafe { (*pointer).settings.create };
    let edit = unsafe { GetDlgItem(hwnd, edit_id as i32) };
    let initial = unsafe { path_text(edit) };
    let initial = (!initial.as_os_str().is_empty()).then_some(initial);
    let selected = match (create, edit_id) {
        (false, ID_PATCH_EDIT) => unsafe {
            file_dialog(
                hwnd,
                "パッチファイルを選択",
                PATCH_FILTER,
                initial.as_deref(),
                false,
                None,
            )
        },
        (false, ID_SOURCE_EDIT) | (true, ID_SOURCE_EDIT) | (true, ID_TARGET_EDIT) => unsafe {
            file_dialog(
                hwnd,
                if !create {
                    "適用先のファイルを選択"
                } else if edit_id == ID_SOURCE_EDIT {
                    "変更前のファイルを選択"
                } else {
                    "変更後のファイルを選択"
                },
                ROM_FILTER,
                initial.as_deref(),
                false,
                None,
            )
        },
        (false, ID_TARGET_EDIT) => unsafe {
            file_dialog(
                hwnd,
                "保存先を選択",
                ALL_FILTER,
                initial.as_deref(),
                true,
                None,
            )
        },
        (true, ID_PATCH_EDIT) => {
            let format_control = unsafe { (*pointer).controls.format };
            let format =
                format_from_index(unsafe { SendMessageW(format_control, CB_GETCURSEL, 0, 0) });
            let filter = format!(
                "{format}パッチ (*.{0})\0*.{0}\0すべてのファイル (*.*)\0*.*\0\0",
                format.extension()
            );
            unsafe {
                file_dialog(
                    hwnd,
                    "保存先を選択",
                    &filter,
                    initial.as_deref(),
                    true,
                    Some(format.extension()),
                )
            }
        }
        _ => return,
    };
    match selected {
        Ok(Some(path)) => unsafe { set_path_text(edit, &path) },
        Ok(None) => {}
        Err(error) => message_box(hwnd, &error, MB_ICONERROR),
    }
}

unsafe fn execute(hwnd: HWND) {
    unsafe { collect_current(hwnd) };
    let pointer = unsafe { state_ptr(hwnd) };
    if pointer.is_null() {
        return;
    }
    let (create, default_path) = unsafe {
        let state = &mut *pointer;
        let default = if state.settings.create {
            if state.settings.create_patch.as_os_str().is_empty()
                && !state.settings.target.as_os_str().is_empty()
            {
                state.settings.create_patch = state
                    .settings
                    .target
                    .with_extension(state.settings.format.extension());
                Some(state.settings.create_patch.clone())
            } else {
                None
            }
        } else if state.settings.output.as_os_str().is_empty()
            && !state.settings.apply_patch.as_os_str().is_empty()
            && !state.settings.source.as_os_str().is_empty()
        {
            state.settings.output =
                default_apply_path(&state.settings.apply_patch, &state.settings.source);
            Some(state.settings.output.clone())
        } else {
            None
        };
        (state.settings.create, default)
    };
    if let Some(path) = default_path {
        let edit = unsafe {
            if create {
                (*pointer).controls.patch_edit
            } else {
                (*pointer).controls.target_edit
            }
        };
        unsafe { set_path_text(edit, &path) };
    }

    if let Err(error) = unsafe { save_snapshot(hwnd) } {
        message_box(hwnd, &error, MB_ICONERROR);
    }
    let Some(settings) = (unsafe { snapshot(hwnd) }) else {
        return;
    };
    if settings.create {
        if let Err(error) = validate_create(&settings) {
            message_box(hwnd, &error, MB_ICONERROR);
            return;
        }
        if !confirm_existing(hwnd, &settings.create_patch) {
            return;
        }
        let format = settings.format;
        let source = settings.source;
        let target = settings.target;
        let output = settings.create_patch;
        let displayed_output = output.clone();
        let result = unsafe {
            run_worker(hwnd, move || {
                create_files(format, &source, &target, &output)
            })
        };
        match result {
            Ok(warnings) => {
                show_result(hwnd, "パッチを作成しました。", &displayed_output, &warnings)
            }
            Err(error) => message_box(hwnd, &error, MB_ICONERROR),
        }
    } else {
        if let Err(error) = validate_apply(&settings) {
            message_box(hwnd, &error, MB_ICONERROR);
            return;
        }
        if !confirm_existing(hwnd, &settings.output) {
            return;
        }
        let patch = settings.apply_patch;
        let source = settings.source;
        let output = settings.output;
        let displayed_output = output.clone();
        let result =
            unsafe { run_worker(hwnd, move || apply_files(&patch, &source, Some(&output))) };
        match result {
            Ok(warnings) => {
                show_result(hwnd, "パッチを適用しました。", &displayed_output, &warnings)
            }
            Err(error) => message_box(hwnd, &error, MB_ICONERROR),
        }
    }
}

fn validate_apply(settings: &GuiSettings) -> Result<(), String> {
    validate_input(&settings.apply_patch, "パッチファイル")?;
    validate_input(&settings.source, "適用先ファイル")?;
    validate_output(&settings.output)
}

fn validate_create(settings: &GuiSettings) -> Result<(), String> {
    validate_input(&settings.source, "変更前ファイル")?;
    validate_input(&settings.target, "変更後ファイル")?;
    validate_output(&settings.create_patch)
}

fn validate_input(path: &Path, label: &str) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err(format!("{label}を指定してください。"));
    }
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("{label}を開けません: {}\n{error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "{label}にファイルを指定してください: {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_output(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("保存先ファイルを指定してください。".into());
    }
    if path.try_exists().map_err(|error| error.to_string())? && !path.is_file() {
        return Err(format!(
            "保存先にファイル名を指定してください: {}",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(format!(
            "保存先フォルダーが存在しません: {}",
            parent.display()
        ));
    }
    Ok(())
}

fn confirm_existing(owner: HWND, output: &Path) -> bool {
    match output.try_exists() {
        Ok(false) => true,
        Ok(true) => {
            let text = wide(&format!(
                "次のファイルを上書きします。よろしいですか？\n{}",
                output.display()
            ));
            let title = wide("iBPS");
            unsafe {
                MessageBoxW(
                    owner,
                    text.as_ptr(),
                    title.as_ptr(),
                    MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
                ) == IDYES
            }
        }
        Err(error) => {
            message_box(
                owner,
                &format!("保存先を確認できません: {error}"),
                MB_ICONERROR,
            );
            false
        }
    }
}

unsafe fn run_worker<F>(owner: HWND, job: F) -> Result<Vec<String>, String>
where
    F: FnOnce() -> Result<Vec<String>, String> + Send + 'static,
{
    let instance = unsafe { GetModuleHandleW(null()) };
    let pointer = unsafe { state_ptr(owner) };
    let (dpi, font) = if pointer.is_null() {
        (96, null_mut())
    } else {
        unsafe { ((*pointer).dpi, (*pointer).font) }
    };
    let width = scale(280, dpi);
    let height = scale(82, dpi);
    let mut owner_rect = RECT::default();
    unsafe { GetWindowRect(owner, &mut owner_rect) };
    let x = owner_rect.left + (owner_rect.right - owner_rect.left - width) / 2;
    let y = owner_rect.top + (owner_rect.bottom - owner_rect.top - height) / 2;
    let processing_class = wide("ibps.processing.window");
    let title = wide("iBPS");
    let processing = unsafe {
        CreateWindowExW(
            0,
            processing_class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_CAPTION,
            x,
            y,
            width,
            height,
            owner,
            null_mut(),
            instance,
            null(),
        )
    };
    if processing.is_null() {
        return Err("処理中ウィンドウを表示できませんでした。".into());
    }
    let status = unsafe {
        CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("処理中…").as_ptr(),
            WS_CHILD | WS_VISIBLE | 1,
            scale(15, dpi),
            scale(14, dpi),
            scale(250, dpi),
            scale(22, dpi),
            processing,
            null_mut(),
            instance,
            null(),
        )
    };
    if status.is_null() {
        unsafe { DestroyWindow(processing) };
        return Err("処理中ウィンドウを表示できませんでした。".into());
    }
    if !font.is_null() {
        unsafe { SendMessageW(status, WM_SETFONT, font as WPARAM, 1) };
    }
    unsafe {
        EnableWindow(owner, 0);
        ShowWindow(processing, SW_SHOW);
        UpdateWindow(processing);
    }

    let (sender, receiver) = mpsc::channel();
    let processing_value = processing as usize;
    let worker = match std::thread::Builder::new()
        .name("patch-operation".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                .unwrap_or_else(|_| Err("処理中に内部エラーが発生しました。".into()));
            let _ = sender.send(result);
            unsafe { PostMessageW(processing_value as HWND, WM_WORK_DONE, 0, 0) };
        }) {
        Ok(worker) => worker,
        Err(error) => {
            unsafe {
                DestroyWindow(processing);
                EnableWindow(owner, 1);
            }
            return Err(format!("処理スレッドを開始できませんでした: {error}"));
        }
    };

    let mut result = Err("処理の完了通知を受け取れませんでした。".into());
    let mut msg = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if status <= 0 {
            if status == 0 {
                unsafe { PostQuitMessage(msg.wParam as i32) };
            }
            break;
        }
        if msg.hwnd == processing && msg.message == WM_WORK_DONE {
            result = receiver
                .recv()
                .unwrap_or_else(|_| Err("処理結果を受け取れませんでした。".into()));
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let _ = worker.join();
    unsafe {
        DestroyWindow(processing);
        EnableWindow(owner, 1);
        if IsWindowVisible(owner) != 0 {
            SetForegroundWindow(owner);
        }
    }
    result
}

unsafe extern "system" fn processing_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_CLOSE {
        0
    } else {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }
}

unsafe fn file_dialog(
    owner: HWND,
    title: &str,
    filter: &str,
    initial: Option<&Path>,
    save: bool,
    default_extension: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let title = wide(title);
    let filter = wide(filter);
    let extension = default_extension.map(wide);
    let mut file = vec![0u16; FILE_BUFFER_LEN];
    if let Some(initial) = initial {
        let value = wide_os(initial.as_os_str());
        if value.len() > file.len() {
            return Err("ファイルパスが長すぎます。".into());
        }
        file[..value.len()].copy_from_slice(&value);
    }
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: extension.as_ref().map_or(null(), |value| value.as_ptr()),
        Flags: OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if !save {
        ofn.Flags |= OFN_FILEMUSTEXIST;
    }
    let accepted = unsafe {
        if save {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        }
    };
    if accepted == 0 {
        let code = unsafe { CommDlgExtendedError() };
        return if code == 0 {
            Ok(None)
        } else {
            Err(format!(
                "ファイルダイアログを開けませんでした (0x{code:08X})。"
            ))
        };
    }
    let length = file
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(file.len());
    Ok(Some(PathBuf::from(OsString::from_wide(&file[..length]))))
}

unsafe fn save_snapshot(hwnd: HWND) -> Result<(), String> {
    let settings = unsafe { snapshot(hwnd) }.ok_or("設定を取得できませんでした。")?;
    settings_win::save(&settings).map_err(|error| format!("設定を保存できませんでした。\n{error}"))
}

unsafe fn snapshot(hwnd: HWND) -> Option<GuiSettings> {
    let pointer = unsafe { state_ptr(hwnd) };
    if pointer.is_null() {
        return None;
    }
    let settings = unsafe { &(*pointer).settings };
    Some(GuiSettings {
        create: settings.create,
        format: settings.format,
        source: settings.source.clone(),
        target: settings.target.clone(),
        output: settings.output.clone(),
        apply_patch: settings.apply_patch.clone(),
        create_patch: settings.create_patch.clone(),
    })
}

unsafe fn state_ptr(hwnd: HWND) -> *mut UiState {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut UiState }
}

unsafe fn focus_is_path_edit(hwnd: HWND) -> bool {
    let pointer = unsafe { state_ptr(hwnd) };
    if pointer.is_null() {
        return false;
    }
    let focus = unsafe { GetFocus() };
    let controls = unsafe { &(*pointer).controls };
    focus == controls.patch_edit || focus == controls.source_edit || focus == controls.target_edit
}

unsafe fn path_text(hwnd: HWND) -> PathBuf {
    let length = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut text = vec![0u16; length + 1];
    let copied =
        unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32) }.max(0) as usize;
    PathBuf::from(OsString::from_wide(&text[..copied]))
}

unsafe fn set_path_text(hwnd: HWND, path: &Path) {
    let text = wide_os(path.as_os_str());
    unsafe { SetWindowTextW(hwnd, text.as_ptr()) };
}

fn format_index(format: Format) -> usize {
    match format {
        Format::Bps => 0,
        Format::Ips => 1,
        Format::Ips32 => 2,
        Format::Ups => 3,
    }
}

fn format_from_index(index: isize) -> Format {
    match index {
        1 => Format::Ips,
        2 => Format::Ips32,
        3 => Format::Ups,
        _ => Format::Bps,
    }
}

fn scale(value: i32, dpi: u32) -> i32 {
    (i64::from(value) * i64::from(dpi) / 96) as i32
}

unsafe fn associate(owner: HWND) {
    match unsafe { register_associations() } {
        Ok(()) => message_box(
            owner,
            ".ips、.ups、.bpsをiBPSに関連付けました。",
            MB_ICONINFORMATION,
        ),
        Err(error) => message_box(owner, &error, MB_ICONERROR),
    }
}

fn show_result(owner: HWND, summary: &str, output: &Path, warnings: &[String]) {
    let mut text = format!("{summary}\n{}", output.display());
    let icon = if warnings.is_empty() {
        MB_ICONINFORMATION
    } else {
        text.push_str("\n\n警告:\n");
        text.push_str(&warnings.join("\n"));
        MB_ICONWARNING
    };
    message_box(owner, &text, icon);
}

unsafe fn register_associations() -> Result<(), String> {
    let prog_id = wide("ibps.patch");
    for extension in [".ips", ".ups", ".bps"] {
        set_registry_default(&wide(&format!("Software\\Classes\\{extension}")), &prog_id)?;
    }
    set_registry_default(
        &wide("Software\\Classes\\ibps.patch"),
        &wide("iBPS パッチファイル"),
    )?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("実行ファイルの場所を取得できませんでした: {error}"))?;
    let mut command = vec![b'"' as u16];
    command.extend(executable.as_os_str().encode_wide());
    command.extend("\" \"%1\"".encode_utf16());
    command.push(0);
    set_registry_default(
        &wide("Software\\Classes\\ibps.patch\\shell\\open\\command"),
        &command,
    )?;
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, null(), null()) };
    Ok(())
}

fn set_registry_default(key_path: &[u16], value: &[u16]) -> Result<(), String> {
    let byte_length = value
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| "関連付け情報が長すぎます。".to_string())?;
    let mut key: HKEY = null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(registry_error("関連付け情報を作成できませんでした", status));
    }
    let status =
        unsafe { RegSetValueExW(key, null(), 0, REG_SZ, value.as_ptr().cast(), byte_length) };
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS {
        return Err(registry_error("関連付け情報を書き込めませんでした", status));
    }
    Ok(())
}

fn registry_error(message: &str, code: u32) -> String {
    format!(
        "{message}: {}",
        std::io::Error::from_raw_os_error(code as i32)
    )
}

fn message_box(owner: HWND, message: &str, icon: u32) {
    let title = wide("iBPS");
    let message = wide(message);
    unsafe { MessageBoxW(owner, message.as_ptr(), title.as_ptr(), MB_OK | icon) };
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn wide_os(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_within_monitor_work_area() {
        let primary = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(centered_origin(&primary, 480, 200), (720, 420));
        let left_monitor = RECT {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 840,
        };
        assert_eq!(centered_origin(&left_monitor, 480, 200), (-1200, 220));
        let left_taskbar = RECT {
            left: 80,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(centered_origin(&left_taskbar, 480, 200), (760, 440));
        assert_eq!(centered_origin(&left_taskbar, 2000, 1200), (80, 0));
    }

    #[test]
    fn association_uses_current_patch_and_selected_source_for_output() {
        let mut settings = GuiSettings {
            create: true,
            source: r"C:\ROM\previous.gba".into(),
            output: r"C:\ROM\do-not-overwrite.gba".into(),
            create_patch: r"C:\ROM\created.ups".into(),
            ..Default::default()
        };
        prepare_associated(&mut settings, r"D:\配布\今回のパッチ.ips".into());
        assert!(!settings.create);
        assert_eq!(settings.output, Path::new(r"C:\ROM\do-not-overwrite.gba"));
        assert_eq!(settings.source, Path::new(r"C:\ROM\previous.gba"));
        let proposed =
            select_associated_source(&mut settings, r"C:\ROM\選択したファイル.bin".into());
        assert_eq!(proposed, Path::new(r"D:\配布\今回のパッチ.bin"));
        assert_eq!(settings.output, Path::new(r"C:\ROM\do-not-overwrite.gba"));
        assert_eq!(settings.create_patch, Path::new(r"C:\ROM\created.ups"));
    }
}
