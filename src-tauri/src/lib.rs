use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use enigo::{Direction, Enigo, Key, Keyboard, Settings as EnigoSettings};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[cfg(windows)]
mod win32 {
    #[link(name = "user32")]
    extern "system" {
        pub fn GetForegroundWindow() -> isize;
        pub fn SetForegroundWindow(hwnd: isize) -> i32;
        pub fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        pub fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: i32) -> i32;
        pub fn GetDC(hwnd: isize) -> isize;
        pub fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
        pub fn GetSystemMetrics(index: i32) -> i32;
        pub fn GetAsyncKeyState(vkey: i32) -> i16;
    }
    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetCurrentThreadId() -> u32;
        pub fn GetCurrentProcessId() -> u32;
    }
    #[link(name = "dwmapi")]
    extern "system" {
        pub fn DwmSetWindowAttribute(
            hwnd: isize,
            attr: u32,
            val: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    pub fn foreground_hwnd() -> isize {
        unsafe { GetForegroundWindow() }
    }

    pub fn foreground_is_current_process() -> bool {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd == 0 {
                return false;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            pid == GetCurrentProcessId()
        }
    }

    pub fn left_mouse_down() -> bool {
        const VK_LBUTTON: i32 = 0x01;
        unsafe { GetAsyncKeyState(VK_LBUTTON) < 0 }
    }

    pub fn ui_automation_selection() -> Option<String> {
        use windows::core::Interface;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
        };

        unsafe {
            if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
                return None;
            }
            let result = (|| {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
                let element = automation.GetFocusedElement().ok()?;
                if element.CurrentIsPassword().ok()?.as_bool() {
                    return None;
                }
                let pattern = element.GetCurrentPattern(UIA_TextPatternId).ok()?;
                let text_pattern: IUIAutomationTextPattern = pattern.cast().ok()?;
                let ranges = text_pattern.GetSelection().ok()?;
                let length = ranges.Length().ok()?;
                let mut selected = Vec::new();
                for index in 0..length {
                    let range = ranges.GetElement(index).ok()?;
                    let text = range.GetText(-1).ok()?.to_string();
                    if !text.trim().is_empty() {
                        selected.push(text);
                    }
                }
                let value = selected.join("\n").trim().to_string();
                (!value.is_empty()).then_some(value)
            })();
            CoUninitialize();
            result
        }
    }

    pub fn refocus(hwnd: isize) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            let cur = GetCurrentThreadId();
            let mut pid: u32 = 0;
            let tid = GetWindowThreadProcessId(hwnd, &mut pid);
            if tid != 0 {
                AttachThreadInput(cur, tid, 1);
            }
            SetForegroundWindow(hwnd);
            if tid != 0 {
                AttachThreadInput(cur, tid, 0);
            }
        }
    }

    pub fn round_corners(hwnd: isize) {
        let pref: u32 = 2;
        unsafe {
            DwmSetWindowAttribute(hwnd, 33, &pref as *const u32 as *const _, 4);
        }
    }

    #[link(name = "gdi32")]
    extern "system" {
        pub fn CreateCompatibleDC(hdc: isize) -> isize;
        pub fn CreateCompatibleBitmap(hdc: isize, w: i32, h: i32) -> isize;
        pub fn SelectObject(hdc: isize, obj: isize) -> isize;
        pub fn BitBlt(
            dst: isize,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            src: isize,
            x1: i32,
            y1: i32,
            rop: u32,
        ) -> i32;
        pub fn GetDIBits(
            hdc: isize,
            hbm: isize,
            start: u32,
            lines: u32,
            bits: *mut core::ffi::c_void,
            bi: *mut core::ffi::c_void,
            usage: u32,
        ) -> i32;
        pub fn DeleteObject(obj: isize) -> i32;
        pub fn DeleteDC(hdc: isize) -> i32;
    }

    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_ppm: i32,
        y_ppm: i32,
        clr_used: u32,
        clr_important: u32,
    }

    const SM_XVIRTUALSCREEN: i32 = 76;
    const SM_YVIRTUALSCREEN: i32 = 77;
    const SM_CXVIRTUALSCREEN: i32 = 78;
    const SM_CYVIRTUALSCREEN: i32 = 79;
    const SRCCOPY: u32 = 0x00CC_0020;
    const CAPTUREBLT: u32 = 0x4000_0000;

    /// 截取整个虚拟桌面（所有显示器一张图），返回 (原点x, 原点y, 宽, 高, RGBA 像素)。
    /// GDI 直接拿设备像素，多屏/混合 DPI 由系统合成，比逐屏拼接更稳。
    /// 仅作 DXGI 的回退：远程桌面 / 安全桌面 / 独占全屏无法 Duplicate 时用它兜底。
    fn capture_virtual_screen_gdi() -> Option<(i32, i32, i32, i32, Vec<u8>)> {
        unsafe {
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            if vw <= 0 || vh <= 0 {
                return None;
            }
            let screen = GetDC(0);
            if screen == 0 {
                return None;
            }
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, vw, vh);
            let old = SelectObject(mem, bmp);
            let blit = BitBlt(mem, 0, 0, vw, vh, screen, vx, vy, SRCCOPY | CAPTUREBLT);

            let mut buf = vec![0u8; (vw as usize) * (vh as usize) * 4];
            let mut bi = BitmapInfoHeader {
                size: core::mem::size_of::<BitmapInfoHeader>() as u32,
                width: vw,
                height: -vh, // 负 = 自上而下，行序正常
                planes: 1,
                bit_count: 32,
                compression: 0, // BI_RGB
                size_image: 0,
                x_ppm: 0,
                y_ppm: 0,
                clr_used: 0,
                clr_important: 0,
            };
            let lines = GetDIBits(
                mem,
                bmp,
                0,
                vh as u32,
                buf.as_mut_ptr() as *mut _,
                &mut bi as *mut _ as *mut _,
                0, // DIB_RGB_COLORS
            );

            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(0, screen);

            if blit == 0 || lines == 0 {
                return None;
            }
            // BGRX -> RGBA
            for px in buf.chunks_exact_mut(4) {
                px.swap(0, 2);
                px[3] = 255;
            }
            Some((vx, vy, vw, vh, buf))
        }
    }

    /// 截取整个虚拟桌面，返回 (原点x, 原点y, 宽, 高, RGBA)。
    /// 优先 DXGI Desktop Duplication（能抓 GPU/独占全屏游戏/视频/硬件覆盖层，避免黑块），
    /// 任一环节异常则回退 GDI（远程桌面、安全桌面、异常适配器等）。契约与旧实现一致。
    pub fn capture_virtual_screen() -> Option<(i32, i32, i32, i32, Vec<u8>)> {
        if let Some(v) = capture_virtual_screen_dxgi() {
            return Some(v);
        }
        capture_virtual_screen_gdi()
    }

    struct DxgiGrab {
        left: i32,
        top: i32,
        w: i32,
        h: i32,
        bgra: Vec<u8>, // 紧凑 BGRA，行宽 = w*4
    }

    /// DXGI Desktop Duplication 抓全部已连接输出并合成为整块虚拟桌面。
    /// “全有或全无”：任一 attached 输出旋转 / 非 BGRA8 / 抓取失败即整体放弃回退 GDI，
    /// 保证要么完整、要么干净回退，绝不残缺拼接。
    fn capture_virtual_screen_dxgi() -> Option<(i32, i32, i32, i32, Vec<u8>)> {
        use windows::core::Interface;
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
        };
        use windows::Win32::Graphics::Dxgi::*;

        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut grabs: Vec<DxgiGrab> = Vec::new();

            let mut ai = 0u32;
            loop {
                let adapter = match factory.EnumAdapters1(ai) {
                    Ok(a) => a,
                    Err(_) => break, // 适配器枚举结束
                };
                ai += 1;

                // 为该适配器建 D3D11 设备（DuplicateOutput 要求设备与输出同适配器）
                let mut device: Option<ID3D11Device> = None;
                let mut context: Option<ID3D11DeviceContext> = None;
                let dev_hr = D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_FLAG(0),
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                );

                let mut oi = 0u32;
                loop {
                    let output = match adapter.EnumOutputs(oi) {
                        Ok(o) => o,
                        Err(_) => break, // 该适配器输出枚举结束
                    };
                    oi += 1;
                    let desc = match output.GetDesc() {
                        Ok(d) => d,
                        Err(_) => return None,
                    };
                    if !desc.AttachedToDesktop.as_bool() {
                        continue; // 未连桌面的输出忽略
                    }
                    // 这块屏连着桌面 —— 必须成功抓取，否则整体回退保证完整
                    if dev_hr.is_err() {
                        return None;
                    }
                    if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
                        && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED
                    {
                        return None; // 旋转屏交给 GDI
                    }
                    let device_ref = device.as_ref()?;
                    let context_ref = context.as_ref()?;
                    let output1: IDXGIOutput1 = output.cast().ok()?;
                    let dupl = match output1.DuplicateOutput(device_ref) {
                        Ok(d) => d,
                        Err(_) => return None, // 被独占/无权限 → 回退
                    };
                    let grab = grab_output(device_ref, context_ref, &dupl, &desc);
                    let _ = dupl.ReleaseFrame();
                    match grab {
                        Some(g) => grabs.push(g),
                        None => return None,
                    }
                }
            }

            if grabs.is_empty() {
                return None;
            }
            let left = grabs.iter().map(|g| g.left).min()?;
            let top = grabs.iter().map(|g| g.top).min()?;
            let right = grabs.iter().map(|g| g.left + g.w).max()?;
            let bottom = grabs.iter().map(|g| g.top + g.h).max()?;
            let vw = right - left;
            let vh = bottom - top;
            if vw <= 0 || vh <= 0 {
                return None;
            }

            let mut buf = vec![0u8; (vw as usize) * (vh as usize) * 4];
            for px in buf.chunks_exact_mut(4) {
                px[3] = 255; // 非矩形虚拟桌面的空隙填黑不透明
            }
            for g in &grabs {
                let ox = (g.left - left) as usize;
                let oy = (g.top - top) as usize;
                let gw = g.w as usize;
                for row in 0..g.h as usize {
                    let src = &g.bgra[row * gw * 4..row * gw * 4 + gw * 4];
                    let dst_off = ((oy + row) * vw as usize + ox) * 4;
                    let dst = &mut buf[dst_off..dst_off + gw * 4];
                    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                        d[0] = s[2]; // 源 BGRA → 目标 RGBA
                        d[1] = s[1];
                        d[2] = s[0];
                        d[3] = 255;
                    }
                }
            }
            // 安全网：合成结果整幅纯黑（DXGI 异常/表面无效）时放弃，改用 GDI，杜绝黑屏回归。
            // 正常画面 top-left 一般即有内容，any() 会立即短路返回。
            let has_content = buf
                .chunks_exact(4)
                .any(|px| px[0] != 0 || px[1] != 0 || px[2] != 0);
            if !has_content {
                return None;
            }
            Some((left, top, vw, vh, buf))
        }
    }

    /// 抓单个输出当前帧 → 拷到 CPU 可读 staging 纹理 → 读出紧凑 BGRA。
    unsafe fn grab_output(
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
        context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
        dupl: &windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
        out_desc: &windows::Win32::Graphics::Dxgi::DXGI_OUTPUT_DESC,
    ) -> Option<DxgiGrab> {
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };
        use windows::Win32::Graphics::Dxgi::{
            IDXGIResource, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
        };

        // 只接受“确有桌面刷新”的帧：首帧 / 仅指针更新的帧（LastPresentTime==0）表面可能空白，
        // 直接用会截到黑屏（DXGI Desktop Duplication 经典坑），必须释放后重试。
        // 活动内容（视频/游戏）每帧都 present，几乎即时命中；静止桌面等到 deadline 便回退 GDI。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut resource: Option<IDXGIResource> = None;
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut ok = false;
        loop {
            match dupl.AcquireNextFrame(120, &mut info, &mut resource) {
                Ok(_) => {
                    if info.LastPresentTime != 0 {
                        ok = true;
                        break;
                    }
                    // 仅元数据/指针帧，表面无效：释放后重试
                    let _ = dupl.ReleaseFrame();
                    resource = None;
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
                Err(_) => return None,
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
        }
        if !ok {
            return None;
        }
        let resource = resource?;
        let tex: ID3D11Texture2D = resource.cast().ok()?;
        let mut td = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut td);
        if td.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return None; // 非 BGRA8（如 HDR fp16）交给 GDI
        }

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: td.Width,
            Height: td.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: td.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .ok()?;
        let staging = staging?;
        context.CopyResource(&staging, &tex);

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .ok()?;

        let w = td.Width as usize;
        let h = td.Height as usize;
        let pitch = mapped.RowPitch as usize;
        let mut bgra = vec![0u8; w * h * 4];
        let src = mapped.pData as *const u8;
        for y in 0..h {
            std::ptr::copy_nonoverlapping(
                src.add(y * pitch),
                bgra.as_mut_ptr().add(y * w * 4),
                w * 4,
            );
        }
        context.Unmap(&staging, 0);

        Some(DxgiGrab {
            left: out_desc.DesktopCoordinates.left,
            top: out_desc.DesktopCoordinates.top,
            w: w as i32,
            h: h as i32,
            bgra,
        })
    }
}

/// Windows 原生 OCR（Windows.Media.Ocr），无需 Python / RapidOCR
#[cfg(windows)]
mod ocr {
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    /// 保证当前线程处于多线程套间（WinRT 激活 + IAsyncOperation::get 阻塞都需要）
    fn init_mta() {
        // 已初始化会返回 S_FALSE / RPC_E_CHANGED_MODE，忽略即可；不主动 uninit
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
    }

    /// 把应用的源语言代码映射成优先尝试的 OCR BCP-47 语言标签。
    /// 注意 en 也优先中文引擎：中文引擎能同时识别中英混排，而英文引擎看到中文
    /// 会输出乱码（实测灾难级）；源语言设 en 的中文用户截中文图是常态，不能炸。
    fn lang_prefs(source_lang: &str) -> Vec<&'static str> {
        match source_lang.trim() {
            "" | "auto" => vec!["zh-Hans", "en"],
            "zh" => vec!["zh-Hans", "en"],
            "zh-Hant" => vec!["zh-Hant", "zh-Hans"],
            "en" => vec!["zh-Hans", "en"],
            "ja" => vec!["ja"],
            "ko" => vec!["ko"],
            "fr" => vec!["fr"],
            "de" => vec!["de"],
            "es" => vec!["es"],
            "it" => vec!["it"],
            "pt" => vec!["pt"],
            "ru" => vec!["ru"],
            "ar" => vec!["ar"],
            "hi" => vec!["hi"],
            "vi" => vec!["vi"],
            "th" => vec!["th"],
            "tr" => vec!["tr"],
            "nl" => vec!["nl"],
            "pl" => vec!["pl"],
            "el" => vec!["el"],
            "sv" => vec!["sv"],
            _ => vec!["en"],
        }
    }

    /// 在系统已安装的 OCR 语言里挑一个最匹配源语言的
    fn pick_language(prefs: &[&str]) -> Result<Language, String> {
        let avail = OcrEngine::AvailableRecognizerLanguages().map_err(|e| e.to_string())?;
        let list: Vec<Language> = avail.into_iter().collect();
        if list.is_empty() {
            return Err(
                "系统未安装 OCR 语言包。请到 设置→时间和语言→语言和区域→对应语言→语言选项，安装“光学字符识别 (OCR)”功能"
                    .into(),
            );
        }
        let tags: Vec<String> = list
            .iter()
            .map(|l| {
                l.LanguageTag()
                    .map(|s| s.to_string())
                    .unwrap_or_default()
                    .to_lowercase()
            })
            .collect();
        for p in prefs {
            let pl = p.to_lowercase();
            for (i, t) in tags.iter().enumerate() {
                if t == &pl || t.starts_with(&pl) || pl.starts_with(t.as_str()) {
                    return Ok(list[i].clone());
                }
            }
        }
        Ok(list[0].clone())
    }

    /// 估计整图背景是否偏暗（深色模式的浅字深底）。文字通常稀疏，故整图均值≈背景亮度。
    /// 抽样计算以省时。均值低于阈值即判为暗底。
    fn dark_background(img: &image::RgbaImage) -> bool {
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return false;
        }
        let step = (w.max(h) / 240).max(1);
        let (mut sum, mut n) = (0u64, 0u64);
        let mut y = 0;
        while y < h {
            let mut x = 0;
            while x < w {
                let p = img.get_pixel(x, y);
                sum += (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as u64;
                n += 1;
                x += step;
            }
            y += step;
        }
        n > 0 && sum / n < 120
    }

    /// 对比度拉伸：灰字灰底 / 半透明覆盖层这类低对比截图是漏字重灾区。
    /// 取亮度 2%~98% 分位做线性拉伸；范围已足够宽（正常黑白文本）或文字
    /// 占比过低（分位全落在背景上）时自动跳过，不影响正常截图。
    fn stretch_contrast(img: &mut image::RgbaImage) {
        let mut hist = [0u32; 256];
        for p in img.pixels() {
            let y = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as usize;
            hist[y.min(255)] += 1;
        }
        let total: u32 = hist.iter().sum();
        if total == 0 {
            return;
        }
        let target = (total / 50).max(1); // 2%
        let (mut lo, mut hi) = (0usize, 255usize);
        let mut acc = 0u32;
        for (i, n) in hist.iter().enumerate() {
            acc += n;
            if acc >= target {
                lo = i;
                break;
            }
        }
        acc = 0;
        for (i, n) in hist.iter().enumerate().rev() {
            acc += n;
            if acc >= target {
                hi = i;
                break;
            }
        }
        if hi <= lo + 8 || hi - lo > 200 {
            return; // 退化（近纯色）或对比度本来就够
        }
        let scale = 255.0 / (hi - lo) as f32;
        let lut: Vec<u8> = (0..256)
            .map(|v| ((v as f32 - lo as f32) * scale).clamp(0.0, 255.0) as u8)
            .collect();
        for p in img.pixels_mut() {
            p[0] = lut[p[0] as usize];
            p[1] = lut[p[1] as usize];
            p[2] = lut[p[2] as usize];
        }
    }

    /// 是否 CJK 及全角标点/假名/谚文（这些字之间不该有空格）
    fn is_cjkish(c: char) -> bool {
        let u = c as u32;
        (0x3400..=0x9FFF).contains(&u)      // CJK 扩展A + 基本汉字
            || (0xF900..=0xFAFF).contains(&u) // 兼容汉字
            || (0x3040..=0x30FF).contains(&u) // 平/片假名
            || (0xAC00..=0xD7A3).contains(&u) // 谚文音节
            || (0x3000..=0x303F).contains(&u) // CJK 标点
            || (0xFF00..=0xFFEF).contains(&u) // 全角/半角形式
    }

    /// Windows OCR 会在 CJK“词”间插空格（"所 以 别"）。合并两侧都是 CJK 的空格，
    /// 保留拉丁文单词间空格（如 "API"、"BYOK key"）。
    fn collapse_cjk_spaces(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == ' ' {
                let mut j = i + 1;
                while j < chars.len() && chars[j] == ' ' {
                    j += 1;
                }
                let prev = out.chars().last();
                let next = chars.get(j).copied();
                let between_cjk =
                    matches!((prev, next), (Some(p), Some(n)) if is_cjkish(p) && is_cjkish(n));
                // 两侧皆 CJK → 去空格；行首/行尾 → 去；否则保留单个空格
                if !between_cjk && prev.is_some() && next.is_some() {
                    out.push(' ');
                }
                i = j;
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        out
    }

    /// 预处理开关（供 A/B 测试与调优；生产走 Default）
    #[derive(Clone, Copy)]
    pub struct PreprocOpts {
        pub stretch: bool,     // 低对比截图做对比度拉伸
        pub short_scale: bool, // 放大倍率额外考虑短边（宽而矮的整行截图）
        pub invert_dark: bool, // 深色底反相为浅底深字
        pub sharpen: bool,     // 放大后 unsharpen 锐化
        pub gray: bool,        // 灰度化（去 ClearType 彩色亚像素边缘）
        pub catmull: bool,     // 用 CatmullRom 插值（否则 Lanczos3，振铃更强）
        pub max_factor: f32,   // 放大倍率上限
    }

    impl Default for PreprocOpts {
        fn default() -> Self {
            Self {
                // A/B 实测（19 张 GDI 渲染样张）：拉伸会破坏抗锯齿渐变，低对比场景反而更差
                stretch: false,
                short_scale: true,
                invert_dark: true,
                sharpen: true,
                gray: false,
                catmull: false,
                // A/B 实测：小字放大超过 ~2x 会被插值撕碎成偏旁（“三步曲”→“卉曲”），
                // 2x 恰好把常见 14px 字推进 Windows OCR 的甜点区（~28px）
                max_factor: 2.0,
            }
        }
    }

    /// 识别一张 RGBA8 图（每像素 R,G,B,A），返回按行拼接的文本
    pub fn recognize_rgba(
        width: u32,
        height: u32,
        rgba: &[u8],
        source_lang: &str,
    ) -> Result<String, String> {
        recognize_with(width, height, rgba, source_lang, PreprocOpts::default())
    }

    pub fn recognize_with(
        width: u32,
        height: u32,
        rgba: &[u8],
        source_lang: &str,
        opts: PreprocOpts,
    ) -> Result<String, String> {
        if width == 0 || height == 0 || rgba.len() < (width as usize) * (height as usize) * 4 {
            return Err("截图数据无效".into());
        }
        init_mta();

        let mut base =
            image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or("截图数据无效")?;
        // 灰度化：中和 ClearType 彩色亚像素边缘（后续反相/放大都在灰度上进行更稳）
        if opts.gray {
            for px in base.pixels_mut() {
                let y = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32) as u8;
                px[0] = y;
                px[1] = y;
                px[2] = y;
            }
        }
        // 深色模式（浅字深底）是 Windows OCR 命中率杀手：暗底整体反相为“深字浅底”再识别
        if opts.invert_dark && dark_background(&base) {
            for px in base.pixels_mut() {
                px[0] = 255 - px[0];
                px[1] = 255 - px[1];
                px[2] = 255 - px[2];
            }
        }
        // 低对比截图先拉伸对比度，再考虑放大
        if opts.stretch {
            stretch_contrast(&mut base);
        }

        // 小图 / 小字放大后 Windows OCR 明显更准。除长边外还看短边：
        // 宽而矮的整行截图（如 2200×60）文字同样小，此前不放大是漏字来源之一
        let long = width.max(height) as f32;
        let short = width.min(height) as f32;
        let f_long = if long < 2000.0 { 2000.0 / long } else { 1.0 };
        let f_short = if opts.short_scale && short < 200.0 {
            200.0 / short
        } else {
            1.0
        };
        let mut factor = f_long.max(f_short).min(opts.max_factor);
        if long * factor > 9000.0 {
            factor = 9000.0 / long; // Windows OCR 位图上限 10000px，留白前先封顶
        }
        let up = if factor > 1.01 {
            let nw = (((width as f32) * factor).round() as u32).max(1);
            let nh = (((height as f32) * factor).round() as u32).max(1);
            let filter = if opts.catmull {
                image::imageops::FilterType::CatmullRom
            } else {
                image::imageops::FilterType::Lanczos3
            };
            let scaled = image::imageops::resize(&base, nw, nh, filter);
            if opts.sharpen {
                // 放大后轻度锐化，让文字边缘更清晰，识别更稳
                image::imageops::unsharpen(&scaled, 1.2, 2)
            } else {
                scaled
            }
        } else {
            base
        };

        // 给文字四周留白（采样四角色作背景）：Windows OCR 对贴边/孤立的短文本
        // 常识别不到或漏掉开头字，加一圈 margin 能显著改善“单字/前两字丢失”
        let (uw, uh) = up.dimensions();
        let pad = ((uw.max(uh) as f32) * 0.12).round().max(36.0) as u32;
        let bgpx = {
            let c = [
                up.get_pixel(0, 0),
                up.get_pixel(uw - 1, 0),
                up.get_pixel(0, uh - 1),
                up.get_pixel(uw - 1, uh - 1),
            ];
            let mut a = [0u32; 3];
            for p in c {
                a[0] += p[0] as u32;
                a[1] += p[1] as u32;
                a[2] += p[2] as u32;
            }
            image::Rgba([(a[0] / 4) as u8, (a[1] / 4) as u8, (a[2] / 4) as u8, 255])
        };
        let mut img = image::RgbaImage::from_pixel(uw + pad * 2, uh + pad * 2, bgpx);
        image::imageops::overlay(&mut img, &up, pad as i64, pad as i64);
        let (w, h) = img.dimensions();

        // RGBA→BGRA，并把 alpha 强制为不透明：剪贴板 DIB 常无有效 alpha，
        // premultiplied 下透明像素会被当成黑色，导致 OCR 直接识别失败
        let mut bgra = img.into_raw();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }

        let buffer = CryptographicBuffer::CreateFromByteArray(&bgra).map_err(|e| e.to_string())?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            w as i32,
            h as i32,
        )
        .map_err(|e| e.to_string())?;

        let lang = pick_language(&lang_prefs(source_lang))?;
        let engine = OcrEngine::TryCreateFromLanguage(&lang).map_err(|e| e.to_string())?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| e.to_string())?
            .get()
            .map_err(|e| e.to_string())?;

        let lines = result.Lines().map_err(|e| e.to_string())?;
        let mut out: Vec<String> = Vec::new();
        for line in lines {
            let raw = line.Text().map(|s| s.to_string()).unwrap_or_default();
            let t = collapse_cjk_spaces(&raw);
            let t = t.trim().to_string();
            if !t.is_empty() {
                out.push(t);
            }
        }
        Ok(out.join("\n"))
    }
}

/// OCR 预处理 A/B 对比：FT_OCR_TEST_DIR 指向 PNG 目录时逐图跑 4 种预处理组合。
/// 运行：FT_OCR_TEST_DIR=... cargo test ocr_ab -- --nocapture
#[cfg(all(test, windows))]
mod ocr_ab_tests {
    use super::ocr;

    #[test]
    fn ocr_ab_matrix() {
        let dir = match std::env::var("FT_OCR_TEST_DIR") {
            Ok(d) => d,
            Err(_) => return, // 未设置则静默跳过（普通 cargo test 不受影响）
        };
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .expect("read test dir")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .map(|e| e.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        let lang = std::env::var("FT_OCR_LANG").unwrap_or_else(|_| "auto".into());
        let old = ocr::PreprocOpts {
            stretch: false,
            short_scale: false,
            max_factor: 6.0,
            ..Default::default()
        };
        let variants: [(&str, ocr::PreprocOpts); 3] = [
            ("pre-today(max6)   ", old),
            (
                "morning(str+short)",
                ocr::PreprocOpts {
                    stretch: true,
                    short_scale: true,
                    max_factor: 6.0,
                    ..old
                },
            ),
            ("current(max2)     ", ocr::PreprocOpts::default()),
        ];
        for p in paths {
            let img = image::open(&p).expect("open png").to_rgba8();
            let (w, h) = img.dimensions();
            let raw = img.into_raw();
            println!(
                "── {} ({w}x{h}) lang={lang}",
                p.file_name().unwrap().to_string_lossy()
            );
            for (label, opts) in variants {
                let out = ocr::recognize_with(w, h, &raw, &lang, opts)
                    .unwrap_or_else(|e| format!("<err: {e}>"));
                println!("  [{label}] {}", out.replace('\n', " ⏎ "));
            }
        }
    }
}

struct AppState {
    target_hwnd: Mutex<isize>,
    snip_full: Mutex<Option<String>>, // F3 全屏截图 data URL，供 snip overlay 拉取
    pin_img: Mutex<Option<String>>,   // 贴图窗口要显示的图片 data URL
    selection_enabled: AtomicBool,
    selection_generation: AtomicU64,
    last_selection: Mutex<String>,
    selection_menu: Mutex<Option<MenuItem<tauri::Wry>>>,
}

fn settings_file(app: &AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("settings.json")
}

fn default_settings() -> Value {
    json!({
        "theme": "light",
        "popupStaySecs": 8,
        "closeOnBlur": false,
        "uiScale": 1.0,
        "sourceLang": "auto",
        "targetLang": "zh",
        "ocr": {
            "mode": "windows"
        },
        "popup": {
            "width": 420,
            "height": 260
        },
        "hotkeys": {
            "f1": "F1",
            "f2": "F2",
            "f3": "F3",
            "f4": "F4",
            "f5": "F5"
        },
        "api": {
            "selected": "default",
            "profiles": [
                { "name": "default", "baseUrl": "", "apiKey": "", "model": "", "supportsVision": false }
            ]
        },
        "prompts": {
            "selected": "",
            "presets": []
        }
    })
}

fn merge_value(base: &mut Value, saved: &Value) {
    match (base, saved) {
        (Value::Object(base_map), Value::Object(saved_map)) => {
            for (k, v) in saved_map {
                if let Some(existing) = base_map.get_mut(k) {
                    merge_value(existing, v);
                } else {
                    base_map.insert(k.clone(), v.clone());
                }
            }
        }
        (base_slot, saved_value) => *base_slot = saved_value.clone(),
    }
}

fn migrate_settings_value(saved: Option<&Value>) -> Value {
    let mut merged = default_settings();
    if let Some(saved) = saved {
        merge_value(&mut merged, saved);
    }
    if let Some(root) = merged.as_object_mut() {
        root.remove("local");
        if let Some(profiles) = root
            .get_mut("api")
            .and_then(Value::as_object_mut)
            .and_then(|api| api.get_mut("profiles"))
            .and_then(Value::as_array_mut)
        {
            for profile in profiles {
                if let Some(profile) = profile.as_object_mut() {
                    profile
                        .entry("supportsVision")
                        .or_insert(Value::Bool(false));
                }
            }
        }
    }
    merged
}

fn read_settings_value(app: &AppHandle) -> Value {
    let saved = std::fs::read_to_string(settings_file(app))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    migrate_settings_value(saved.as_ref())
}

#[tauri::command]
fn settings_get(app: AppHandle) -> Value {
    read_settings_value(&app)
}

#[tauri::command]
fn settings_set(app: AppHandle, value: Value) -> Result<(), String> {
    let value = migrate_settings_value(Some(&value));
    let path = settings_file(&app);
    let raw = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())?;
    let _ = app.emit("settings-changed", value);
    apply_hotkeys(&app);
    Ok(())
}

fn grab_selection_blocking() -> Option<String> {
    use std::borrow::Cow;

    enum ClipboardSnapshot {
        Text(String),
        Image {
            width: usize,
            height: usize,
            bytes: Vec<u8>,
        },
        Empty,
    }

    let (before, snapshot) = match arboard::Clipboard::new() {
        Ok(mut clipboard) => {
            if let Ok(text) = clipboard.get_text() {
                (text.clone(), ClipboardSnapshot::Text(text))
            } else if let Ok(image) = clipboard.get_image() {
                (
                    String::new(),
                    ClipboardSnapshot::Image {
                        width: image.width,
                        height: image.height,
                        bytes: image.bytes.into_owned(),
                    },
                )
            } else {
                (String::new(), ClipboardSnapshot::Empty)
            }
        }
        Err(_) => (String::new(), ClipboardSnapshot::Empty),
    };

    if let Ok(mut enigo) = Enigo::new(&EnigoSettings::default()) {
        let _ = enigo.key(Key::Control, Direction::Press);
        std::thread::sleep(Duration::from_millis(15));
        let _ = enigo.key(Key::Unicode('c'), Direction::Click);
        std::thread::sleep(Duration::from_millis(15));
        let _ = enigo.key(Key::Control, Direction::Release);
    }

    let t0 = Instant::now();
    let selected = loop {
        std::thread::sleep(Duration::from_millis(30));
        let now = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
            .unwrap_or_default();
        if !now.trim().is_empty() && now != before {
            break Some(now);
        }
        if t0.elapsed() > Duration::from_millis(700) {
            break None;
        }
    };

    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        match snapshot {
            ClipboardSnapshot::Text(text) => {
                let _ = clipboard.set_text(text);
            }
            ClipboardSnapshot::Image {
                width,
                height,
                bytes,
            } => {
                let _ = clipboard.set_image(arboard::ImageData {
                    width,
                    height,
                    bytes: Cow::Owned(bytes),
                });
            }
            ClipboardSnapshot::Empty => {
                let _ = clipboard.clear();
            }
        }
    }
    selected
}

fn valid_selection(text: &str) -> Option<String> {
    let normalized = text.replace("\r\n", "\n").trim().to_string();
    let meaningful = normalized.chars().filter(|c| !c.is_whitespace()).count();
    (meaningful >= 2 && meaningful <= 20_000).then_some(normalized)
}

fn generation_is_current(counter: &AtomicU64, generation: u64) -> bool {
    counter.load(Ordering::SeqCst) == generation
}

#[tauri::command]
async fn commit_paste(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<(), String> {
    if let Some(popup) = app.get_webview_window("popup") {
        let _ = popup.hide();
    }
    let hwnd = *state.target_hwnd.lock().unwrap();
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(mut c) = arboard::Clipboard::new() {
            let _ = c.set_text(text);
        }
        std::thread::sleep(Duration::from_millis(120));
        #[cfg(windows)]
        win32::refocus(hwnd);
        std::thread::sleep(Duration::from_millis(60));
        if let Ok(mut enigo) = Enigo::new(&EnigoSettings::default()) {
            let _ = enigo.key(Key::Control, Direction::Press);
            std::thread::sleep(Duration::from_millis(15));
            let _ = enigo.key(Key::Unicode('v'), Direction::Click);
            std::thread::sleep(Duration::from_millis(15));
            let _ = enigo.key(Key::Control, Direction::Release);
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    let mut c = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    c.set_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
fn clipboard_text() -> String {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
        .unwrap_or_default()
}

fn clamp_popup_size(width: f64, height: f64) -> (f64, f64) {
    (width.clamp(320.0, 960.0), height.clamp(180.0, 800.0))
}

fn clamp_popup_position(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    bounds: (f64, f64, f64, f64),
    pad: f64,
) -> (f64, f64) {
    let (left, top, right, bottom) = bounds;
    let max_x = (right - width - pad).max(left + pad);
    let max_y = (bottom - height - pad).max(top + pad);
    (x.clamp(left + pad, max_x), y.clamp(top + pad, max_y))
}

#[tauri::command]
fn popup_present(app: AppHandle, width: f64, height: f64, focus: bool) -> Result<(), String> {
    let popup = app
        .get_webview_window("popup")
        .ok_or("popup window missing")?;
    let settings = read_settings_value(&app);
    let configured = settings.get("popup").and_then(Value::as_object);
    let configured_width = configured
        .and_then(|value| value.get("width"))
        .and_then(Value::as_f64)
        .unwrap_or(width);
    let configured_height = configured
        .and_then(|value| value.get("height"))
        .and_then(Value::as_f64)
        .unwrap_or(height);
    let (width, height) = clamp_popup_size(configured_width, configured_height);
    let cursor = app.cursor_position().map_err(|e| e.to_string())?;
    let monitor = app
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());

    let (scale, mx, my, mw, mh) = match &monitor {
        Some(m) => (
            m.scale_factor(),
            m.position().x as f64,
            m.position().y as f64,
            m.size().width as f64,
            m.size().height as f64,
        ),
        None => (1.0, 0.0, 0.0, 1920.0, 1080.0),
    };

    let wp = width * scale;
    let hp = height * scale;
    let pad = 10.0 * scale;
    let mut x = cursor.x + 12.0 * scale;
    let mut y = cursor.y + 22.0 * scale;
    if x + wp > mx + mw - pad {
        x = mx + mw - wp - pad;
    }
    if y + hp > my + mh - pad {
        y = cursor.y - hp - 16.0 * scale;
    }
    (x, y) = clamp_popup_position(x, y, wp, hp, (mx, my, mx + mw, my + mh), pad);

    let _ = popup.set_size(LogicalSize::new(width, height));
    let _ = popup.set_position(PhysicalPosition::new(x, y));
    let _ = popup.set_focusable(focus);
    let _ = popup.show();
    let _ = popup.set_always_on_top(true);
    if focus {
        let _ = popup.set_focus();
    }
    Ok(())
}

#[tauri::command]
fn popup_resize(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    let (width, height) = clamp_popup_size(width, height);
    if let Some(popup) = app.get_webview_window("popup") {
        let _ = popup.set_size(LogicalSize::new(width, height));
        // 变高后重新夹住位置，避免长译文把底部按钮顶出屏幕（够不到“复制”）
        if let Ok(pos) = popup.outer_position() {
            let scale = popup.scale_factor().unwrap_or(1.0);
            let monitor = popup
                .current_monitor()
                .ok()
                .flatten()
                .or_else(|| app.primary_monitor().ok().flatten());
            if let Some(m) = monitor {
                let mpos = m.position();
                let msize = m.size();
                let wp = width * scale;
                let hp = height * scale;
                let pad = 8.0 * scale;
                let left = mpos.x as f64 + pad;
                let top = mpos.y as f64 + pad;
                let right = mpos.x as f64 + msize.width as f64 - pad;
                let bottom = mpos.y as f64 + msize.height as f64 - pad;
                let (x, y) = clamp_popup_position(
                    pos.x as f64,
                    pos.y as f64,
                    wp,
                    hp,
                    (left - pad, top - pad, right + pad, bottom + pad),
                    pad,
                );
                let _ = popup.set_position(PhysicalPosition::new(x, y));
            }
        }
    }
    Ok(())
}

/// 让 popup 可获焦并聚焦（“点击外部关闭”需要：只有获焦窗口才能感知失焦）
#[tauri::command]
fn popup_grab_focus(app: AppHandle) {
    if let Some(popup) = app.get_webview_window("popup") {
        let _ = popup.set_focusable(true);
        let _ = popup.set_focus();
    }
}

#[derive(Deserialize)]
struct LlmRequest {
    base_url: String,
    api_key: String,
    model: String,
    messages: Vec<Value>,
    temperature: Option<f64>,
}

#[tauri::command]
async fn llm_stream(
    window: tauri::WebviewWindow,
    req_id: u64,
    req: LlmRequest,
) -> Result<(), String> {
    let base = req.base_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        let _ = window.emit(
            "llm:error",
            json!({ "id": req_id, "message": "未配置 API，请在设置中填写 Base URL / Key / Model" }),
        );
        return Ok(());
    }
    // 容错：用户把完整 endpoint 填进 Base URL 时不再重复拼接
    let url = if base.ends_with("/chat/completions") {
        base.clone()
    } else {
        format!("{}/chat/completions", base)
    };
    let body = json!({
        "model": req.model,
        "messages": req.messages,
        "stream": true,
        "temperature": req.temperature.unwrap_or(0.2),
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(req.api_key.trim())
        .json(&body)
        .timeout(Duration::from_secs(120))
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            let _ = window.emit(
                "llm:error",
                json!({ "id": req_id, "message": format!("请求失败：{e}") }),
            );
            return Ok(());
        }
    };

    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let brief: String = text.chars().take(300).collect();
        let _ = window.emit(
            "llm:error",
            json!({ "id": req_id, "message": format!("API 错误 {code}：{brief}") }),
        );
        return Ok(());
    }

    let mut full = String::new();
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = window.emit(
                    "llm:error",
                    json!({ "id": req_id, "message": format!("流中断：{e}") }),
                );
                return Ok(());
            }
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);
            if line.is_empty() || !line.starts_with("data:") {
                continue;
            }
            let payload = line[5..].trim();
            if payload == "[DONE]" {
                let _ = window.emit("llm:done", json!({ "id": req_id, "full": full }));
                return Ok(());
            }
            if let Ok(v) = serde_json::from_str::<Value>(payload) {
                if let Some(delta) = v["choices"][0]["delta"]["content"].as_str() {
                    if !delta.is_empty() {
                        full.push_str(delta);
                        let _ = window.emit("llm:delta", json!({ "id": req_id, "text": delta }));
                    }
                }
            }
        }
    }
    let _ = window.emit("llm:done", json!({ "id": req_id, "full": full }));
    Ok(())
}

#[derive(Deserialize)]
struct VisionOcrRequest {
    #[serde(rename = "baseUrl")]
    base_url: String,
    #[serde(rename = "apiKey")]
    api_key: String,
    model: String,
    #[serde(rename = "imageB64")]
    image_b64: String,
    #[serde(rename = "sourceLang")]
    source_lang: String,
}

fn response_content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.trim().to_string();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
                .trim()
                .to_string()
        })
        .unwrap_or_default()
}

fn is_cjk_char(value: char) -> bool {
    let codepoint = value as u32;
    (0x3400..=0x9FFF).contains(&codepoint)
        || (0xF900..=0xFAFF).contains(&codepoint)
        || (0x3040..=0x30FF).contains(&codepoint)
        || (0xAC00..=0xD7A3).contains(&codepoint)
        || (0x3000..=0x303F).contains(&codepoint)
        || (0xFF00..=0xFFEF).contains(&codepoint)
}

fn normalize_ocr_lines(text: &str) -> String {
    let mut output = String::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if output.is_empty() {
            output.push_str(line);
            continue;
        }
        let previous = output.chars().last().unwrap_or(' ');
        let next = line.chars().next().unwrap_or(' ');
        if matches!(
            previous,
            '。' | '！' | '？' | '!' | '?' | '；' | ';' | '：' | ':'
        ) {
            output.push('\n');
        } else if !(is_cjk_char(previous) || is_cjk_char(next)) {
            output.push(' ');
        }
        output.push_str(line);
    }
    output
}

#[tauri::command]
async fn vision_ocr(req: VisionOcrRequest) -> Result<Value, String> {
    let base = req.base_url.trim().trim_end_matches('/');
    if base.is_empty() || req.model.trim().is_empty() {
        return Err("视觉 OCR 需要完整的 API Base URL 和模型名称".into());
    }
    if !req.image_b64.starts_with("data:image/") {
        return Err("截图数据格式无效".into());
    }
    let url = if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    };
    let lang_hint = if req.source_lang.trim().is_empty() || req.source_lang == "auto" {
        "Detect the source language automatically.".to_string()
    } else {
        format!(
            "The expected source language code is {}.",
            req.source_lang.trim()
        )
    };
    let body = json!({
        "model": req.model,
        "stream": false,
        "temperature": 0,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "text",
                    "text": format!(
                        "Extract all visible text from this screenshot. {lang_hint} Preserve reading order and meaningful line breaks. Output only the extracted text, with no commentary."
                    )
                },
                {
                    "type": "image_url",
                    "image_url": { "url": req.image_b64 }
                }
            ]
        }]
    });
    let resp = reqwest::Client::new()
        .post(url)
        .bearer_auth(req.api_key.trim())
        .json(&body)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("视觉 OCR 请求失败：{e}"))?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let detail: String = resp
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect();
        return Err(format!("视觉 OCR API 错误 {code}：{detail}"));
    }
    let value: Value = resp
        .json()
        .await
        .map_err(|e| format!("视觉 OCR 响应解析失败：{e}"))?;
    let source = response_content_text(&value["choices"][0]["message"]["content"]);
    if source.is_empty() {
        return Err("视觉模型未识别到文字".into());
    }
    Ok(json!({ "source": source }))
}

fn remember_foreground(app: &AppHandle) {
    #[cfg(windows)]
    {
        let hwnd = win32::foreground_hwnd();
        let state = app.state::<AppState>();
        *state.target_hwnd.lock().unwrap() = hwnd;
    }
}

fn refresh_selection_indicator(app: &AppHandle, enabled: bool) {
    let state = app.state::<AppState>();
    if let Some(item) = state.selection_menu.lock().unwrap().as_ref() {
        let _ = item.set_text(if enabled {
            "划词翻译：开启"
        } else {
            "划词翻译：关闭"
        });
    }
    if let Some(tray) = app.tray_by_id("tray") {
        let tooltip = if enabled {
            "FlashTrans - 划词翻译已开启"
        } else {
            "FlashTrans - 划词翻译已关闭"
        };
        let _ = tray.set_tooltip(Some(tooltip));
    }
    let _ = app.emit("selection-mode-changed", enabled);
}

#[tauri::command]
fn selection_mode_get(state: tauri::State<'_, AppState>) -> bool {
    state.selection_enabled.load(Ordering::SeqCst)
}

#[tauri::command]
fn selection_mode_set(app: AppHandle, state: tauri::State<'_, AppState>, enabled: bool) {
    state.selection_enabled.store(enabled, Ordering::SeqCst);
    state.selection_generation.fetch_add(1, Ordering::SeqCst);
    if !enabled {
        state.last_selection.lock().unwrap().clear();
    }
    refresh_selection_indicator(&app, enabled);
}

fn toggle_selection_mode(app: &AppHandle) {
    let state = app.state::<AppState>();
    let enabled = !state.selection_enabled.load(Ordering::SeqCst);
    state.selection_enabled.store(enabled, Ordering::SeqCst);
    state.selection_generation.fetch_add(1, Ordering::SeqCst);
    if !enabled {
        state.last_selection.lock().unwrap().clear();
    }
    refresh_selection_indicator(app, enabled);
}

fn process_mouse_selection(app: AppHandle, generation: u64) {
    std::thread::sleep(Duration::from_millis(160));
    let state = app.state::<AppState>();
    if !state.selection_enabled.load(Ordering::SeqCst)
        || !generation_is_current(&state.selection_generation, generation)
    {
        return;
    }
    #[cfg(windows)]
    if win32::foreground_is_current_process() {
        return;
    }

    #[cfg(windows)]
    let selected = win32::ui_automation_selection().or_else(grab_selection_blocking);
    #[cfg(not(windows))]
    let selected = grab_selection_blocking();
    let Some(text) = selected.as_deref().and_then(valid_selection) else {
        return;
    };

    {
        let mut last = state.last_selection.lock().unwrap();
        if *last == text {
            return;
        }
        *last = text.clone();
    }
    if !generation_is_current(&state.selection_generation, generation) {
        return;
    }

    remember_foreground(&app);
    let _ = popup_present(app.clone(), 420.0, 260.0, false);
    let _ = app.emit_to(
        "popup",
        "popup-mode",
        json!({ "mode": "f1", "state": "text", "text": text, "title": "划词翻译" }),
    );
}

fn start_selection_monitor(app: AppHandle) {
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            let mut was_down = false;
            loop {
                std::thread::sleep(Duration::from_millis(24));
                let down = win32::left_mouse_down();
                if was_down && !down {
                    let state = app.state::<AppState>();
                    if state.selection_enabled.load(Ordering::SeqCst) {
                        let generation =
                            state.selection_generation.fetch_add(1, Ordering::SeqCst) + 1;
                        let app2 = app.clone();
                        std::thread::spawn(move || process_mouse_selection(app2, generation));
                    }
                }
                was_down = down;
            }
        }
    });
}

fn on_f1(app: &AppHandle) {
    toggle_selection_mode(app);
}

fn activate_main_window(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
}

fn on_f2(app: &AppHandle) {
    remember_foreground(app);
    let _ = app.emit_to("popup", "popup-mode", json!({ "mode": "f2" }));
    let _ = popup_present(app.clone(), 420.0, 168.0, true);
}

/// 把 RGBA 像素编码成 PNG 的 data URL（供 snip overlay / 复制截图 / 贴图置顶复用）
fn encode_png_data_url(w: u32, h: u32, rgba: &[u8]) -> String {
    use base64::Engine as _;
    let img = match image::RgbaImage::from_raw(w, h, rgba.to_vec()) {
        Some(i) => i,
        None => return String::new(),
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    if image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .is_err()
    {
        return String::new();
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.get_ref());
    format!("data:image/png;base64,{b64}")
}

fn ocr_popup_error(app: &AppHandle, msg: String) {
    let _ = popup_present(app.clone(), 420.0, 118.0, false);
    let _ = app.emit_to(
        "popup",
        "popup-mode",
        json!({ "mode": "ocr", "error": msg }),
    );
}

/// F3：截取整个虚拟桌面（所有屏一张图）→ 打开覆盖全屏的自定义框选 overlay。
/// 框选、裁剪、OCR 都在自定义窗口内完成，无系统截图工具的声音 / 落盘 / 弹窗。
fn on_f3(app: &AppHandle) {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)]
        let cap = win32::capture_virtual_screen();
        #[cfg(not(windows))]
        let cap: Option<(i32, i32, i32, i32, Vec<u8>)> = None;

        let (vx, vy, vw, vh, rgba) = match cap {
            Some(v) => v,
            None => {
                ocr_popup_error(&app2, "截屏失败".into());
                return;
            }
        };

        let data_url = encode_png_data_url(vw as u32, vh as u32, &rgba);
        if data_url.is_empty() {
            ocr_popup_error(&app2, "截屏编码失败".into());
            return;
        }
        *app2.state::<AppState>().snip_full.lock().unwrap() = Some(data_url);

        if let Some(snip) = app2.get_webview_window("snip") {
            let _ = snip.set_position(PhysicalPosition::new(vx, vy));
            let _ = snip.set_size(tauri::PhysicalSize::new(vw as u32, vh as u32));
            let _ = snip.show();
            let _ = snip.set_focus();
            // 主动信号：snip.ts 收到后调 snip_data 拉取全屏图（避免 emit 早于 webview 就绪）
            let _ = app2.emit_to("snip", "snip-open", json!({}));
        }
    });
}

/// snip overlay 主动拉取当前的全屏截图（data URL）
#[tauri::command]
fn snip_data(state: tauri::State<'_, AppState>) -> Option<String> {
    state.snip_full.lock().unwrap().clone()
}

/// 隐藏 snip overlay（取消或完成时）
#[tauri::command]
fn snip_hide(app: AppHandle) {
    if let Some(w) = app.get_webview_window("snip") {
        let _ = w.hide();
    }
}

#[derive(Deserialize)]
struct NativeOcrRequest {
    #[serde(rename = "imageB64")]
    image_b64: String,
    #[serde(rename = "sourceLang")]
    source_lang: String,
}

/// 截图 OCR：使用 Windows.Media.Ocr，无需 Python 或本地模型。
#[tauri::command]
async fn native_ocr(req: NativeOcrRequest) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        use base64::Engine as _;
        let raw = req.image_b64.split(',').last().unwrap_or("");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .map_err(|e| e.to_string())?;
        let img = image::load_from_memory(&bytes)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let (w, h) = img.dimensions();
        #[cfg(windows)]
        let source = ocr::recognize_rgba(w, h, &img.into_raw(), &req.source_lang)?;
        #[cfg(not(windows))]
        let source = {
            let _ = (w, h);
            String::new()
        };
        Ok(json!({ "source": normalize_ocr_lines(&source) }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 把一张 PNG（base64，可含 data: 前缀）复制到剪贴板
#[tauri::command]
fn copy_image(b64: String) -> Result<(), String> {
    use base64::Engine as _;
    let raw = b64.split(",").last().unwrap_or("");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_image(arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: std::borrow::Cow::from(img.into_raw()),
    })
    .map_err(|e| e.to_string())
}

/// 贴图置顶：把截取的图片钉在屏幕上（可拖动的置顶小窗）
#[tauri::command]
fn pin_show(app: AppHandle, b64: String, width: f64, height: f64) -> Result<(), String> {
    *app.state::<AppState>().pin_img.lock().unwrap() = Some(b64);
    let pin = app.get_webview_window("pin").ok_or("pin window missing")?;
    let cursor = app.cursor_position().map_err(|e| e.to_string())?;
    let w = width.max(80.0).min(1600.0);
    let h = height.max(60.0).min(1200.0);
    let _ = pin.set_size(LogicalSize::new(w, h));
    let _ = pin.set_position(PhysicalPosition::new(cursor.x + 12.0, cursor.y + 12.0));
    let _ = pin.show();
    let _ = pin.set_always_on_top(true);
    let _ = pin.set_focus();
    let _ = app.emit_to("pin", "pin-open", json!({}));
    Ok(())
}

#[tauri::command]
fn pin_data(state: tauri::State<'_, AppState>) -> Option<String> {
    state.pin_img.lock().unwrap().clone()
}

#[tauri::command]
fn pin_close(app: AppHandle) {
    if let Some(w) = app.get_webview_window("pin") {
        let _ = w.hide();
    }
}

/// 在系统文件管理器中打开文件夹（绕过 opener 插件的路径 scope 限制）
#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    let p = path.replace('/', "\\");
    #[cfg(windows)]
    {
        Command::new("explorer")
            .arg(&p)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    {
        let _ = p;
        Err("unsupported platform".into())
    }
}

fn on_f4(app: &AppHandle) {
    let clip = arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
        .unwrap_or_default();
    if let Some(chat) = app.get_webview_window("chat") {
        let _ = chat.show();
        let _ = chat.set_focus();
        let _ = app.emit_to("chat", "chat-open", json!({ "clipboard": clip.trim() }));
    }
}

fn on_f5(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        if main.is_visible().unwrap_or(false) && main.is_focused().unwrap_or(false) {
            let _ = main.hide();
        } else {
            let _ = main.show();
            let _ = main.unminimize();
            let _ = main.set_focus();
        }
    }
}

/// 按设置里的组合键注册全局热键（设置保存时重新注册）
fn apply_hotkeys(app: &AppHandle) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let s = read_settings_value(app);
    let empty = json!({});
    let hk = s.get("hotkeys").unwrap_or(&empty);
    for (name, def) in [
        ("f1", "F1"),
        ("f2", "F2"),
        ("f3", "F3"),
        ("f4", "F4"),
        ("f5", "F5"),
    ] {
        let combo = hk
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or(def)
            .trim()
            .to_string();
        if combo.is_empty() {
            continue;
        }
        if let Err(e) = gs.register(combo.as_str()) {
            eprintln!("hotkey {name}={combo} register failed: {e}");
        }
    }
}

fn dispatch_hotkey(app: &AppHandle, shortcut: &Shortcut) {
    let s = read_settings_value(app);
    let empty = json!({});
    let hk = s.get("hotkeys").unwrap_or(&empty);
    let parse = |name: &str, def: &str| -> Option<Shortcut> {
        hk.get(name)
            .and_then(Value::as_str)
            .unwrap_or(def)
            .parse::<Shortcut>()
            .ok()
    };
    if Some(*shortcut) == parse("f1", "F1") {
        on_f1(app);
    } else if Some(*shortcut) == parse("f2", "F2") {
        on_f2(app);
    } else if Some(*shortcut) == parse("f3", "F3") {
        on_f3(app);
    } else if Some(*shortcut) == parse("f4", "F4") {
        on_f4(app);
    } else if Some(*shortcut) == parse("f5", "F5") {
        on_f5(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_migration_removes_local_and_fills_online_defaults() {
        let saved = json!({
            "theme": "light",
            "local": { "backend": "qwen", "model": "old.gguf" },
            "api": {
                "selected": "existing",
                "profiles": [{
                    "name": "existing",
                    "baseUrl": "https://example.test/v1",
                    "apiKey": "secret",
                    "model": "model-a"
                }]
            }
        });

        let migrated = migrate_settings_value(Some(&saved));

        assert_eq!(migrated["theme"], "light");
        assert!(migrated.get("local").is_none());
        assert_eq!(migrated["ocr"]["mode"], "windows");
        assert_eq!(migrated["popup"]["width"], 420);
        assert_eq!(migrated["api"]["profiles"][0]["apiKey"], "secret");
        assert_eq!(migrated["api"]["profiles"][0]["supportsVision"], false);
    }

    #[test]
    fn vision_response_accepts_string_and_content_parts() {
        assert_eq!(
            response_content_text(&json!("  detected text  ")),
            "detected text"
        );
        assert_eq!(
            response_content_text(&json!([
                { "type": "text", "text": " first" },
                { "type": "output_text", "text": " second " },
                { "type": "image_url", "image_url": { "url": "ignored" } }
            ])),
            "first second"
        );
        assert!(response_content_text(&json!({ "text": "unsupported" })).is_empty());
    }

    #[test]
    fn ocr_lines_join_wrapped_text_but_keep_sentence_breaks() {
        assert_eq!(
            normalize_ocr_lines("Hello\nworld.\nNext line"),
            "Hello world. Next line"
        );
        assert_eq!(
            normalize_ocr_lines("第一行\n第二行。\n第三行"),
            "第一行第二行。\n第三行"
        );
    }

    #[test]
    fn selection_filter_normalizes_and_enforces_limits() {
        assert_eq!(
            valid_selection(" \r\n hello\r\nworld \r\n"),
            Some("hello\nworld".into())
        );
        assert!(valid_selection(" x ").is_none());
        assert!(valid_selection(" \r\n\t ").is_none());
        assert!(valid_selection(&"x".repeat(20_001)).is_none());
        assert_eq!(valid_selection(&"x".repeat(20_000)).unwrap().len(), 20_000);
    }

    #[test]
    fn popup_size_and_position_stay_inside_monitor_bounds() {
        assert_eq!(clamp_popup_size(100.0, 100.0), (320.0, 180.0));
        assert_eq!(clamp_popup_size(1_200.0, 900.0), (960.0, 800.0));
        assert_eq!(
            clamp_popup_position(
                1_900.0,
                1_000.0,
                420.0,
                260.0,
                (0.0, 0.0, 1_920.0, 1_080.0),
                10.0
            ),
            (1_490.0, 810.0)
        );
        assert_eq!(
            clamp_popup_position(
                -3_000.0,
                -100.0,
                420.0,
                260.0,
                (-1_920.0, 0.0, 0.0, 1_080.0),
                10.0
            ),
            (-1_910.0, 10.0)
        );
    }

    #[test]
    fn newer_selection_generation_invalidates_older_work() {
        let counter = AtomicU64::new(7);
        assert!(generation_is_current(&counter, 7));
        let next = counter.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(next, 8);
        assert!(!generation_is_current(&counter, 7));
        assert!(generation_is_current(&counter, next));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            eprintln!("secondary launch detected; activating main window");
            activate_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            target_hwnd: Mutex::new(0),
            snip_full: Mutex::new(None),
            pin_img: Mutex::new(None),
            selection_enabled: AtomicBool::new(false),
            selection_generation: AtomicU64::new(0),
            last_selection: Mutex::new(String::new()),
            selection_menu: Mutex::new(None),
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    dispatch_hotkey(app, shortcut);
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            settings_get,
            settings_set,
            commit_paste,
            copy_text,
            clipboard_text,
            selection_mode_get,
            selection_mode_set,
            popup_present,
            popup_resize,
            popup_grab_focus,
            llm_stream,
            vision_ocr,
            snip_data,
            snip_hide,
            native_ocr,
            copy_image,
            pin_show,
            pin_data,
            pin_close,
            open_folder
        ])
        .setup(|app| {
            apply_hotkeys(app.handle());
            start_selection_monitor(app.handle().clone());

            #[cfg(windows)]
            for label in ["main", "popup", "chat", "pin"] {
                if let Some(w) = app.get_webview_window(label) {
                    if let Ok(h) = w.hwnd() {
                        win32::round_corners(h.0 as isize);
                    }
                }
            }

            let show_item = MenuItemBuilder::with_id("show", "打开仪表盘").build(app)?;
            let selection_item =
                MenuItemBuilder::with_id("selection", "划词翻译：关闭").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "退出 FlashTrans").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show_item)
                .item(&selection_item)
                .separator()
                .item(&quit_item)
                .build()?;
            *app.state::<AppState>().selection_menu.lock().unwrap() = Some(selection_item);
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("FlashTrans - 划词翻译已关闭")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        activate_main_window(app);
                    }
                    "selection" => toggle_selection_mode(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(main) = app.get_webview_window("main") {
                            let _ = main.show();
                            let _ = main.unminimize();
                            let _ = main.set_focus();
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" || window.label() == "chat" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
