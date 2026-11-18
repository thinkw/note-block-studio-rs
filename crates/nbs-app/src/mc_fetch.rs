//! Minecraft 资源下载器：导入音效向导走 HTTP 的两条腿（取文本、取文件）。
//! 原版 `http_get` / `http_get_file` 的底层就是 GameMaker 的 WinINet，这里同路。

use std::io::Write;
use std::path::Path;

use nbs_editor::sound_import::MinecraftFetcher;

/// WinINet 拉一个 URL，要求 HTTP 200 且响应非空；GitHub API 没 UA 一律 403。
pub(crate) fn fetch_bytes(url: &str) -> Result<Vec<u8>, ()> {
    use std::ffi::CString;
    use winapi::um::wininet::*;

    let agent = CString::new("nbs-rust").map_err(|_| ())?;
    let url = CString::new(url).map_err(|_| ())?;
    let headers = CString::new("User-Agent: nbs-rust\r\n").map_err(|_| ())?;
    unsafe {
        let session = InternetOpenA(
            agent.as_ptr(),
            INTERNET_OPEN_TYPE_PRECONFIG,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err(());
        }
        let request = InternetOpenUrlA(
            session,
            url.as_ptr(),
            headers.as_ptr(),
            (headers.as_bytes().len() - 1) as u32,
            INTERNET_FLAG_RELOAD,
            0,
        );
        if request.is_null() {
            InternetCloseHandle(session);
            return Err(());
        }
        let mut code_buf = [0u8; 16];
        let mut len = code_buf.len() as u32;
        let mut index = 0u32;
        let ok = HttpQueryInfoA(
            request,
            HTTP_QUERY_STATUS_CODE,
            code_buf.as_mut_ptr() as *mut winapi::ctypes::c_void,
            &mut len,
            &mut index,
        );
        let status_ok = ok != 0
            && code_buf
                .iter()
                .position(|&b| b == 0)
                .and_then(|n| core::str::from_utf8(&code_buf[..n]).ok())
                .and_then(|s| s.trim().parse::<u32>().ok())
                == Some(200);
        let mut body = Vec::new();
        if status_ok {
            let mut buf = [0u8; 65536];
            loop {
                let mut read = 0u32;
                let ok = InternetReadFile(
                    request,
                    buf.as_mut_ptr() as *mut winapi::ctypes::c_void,
                    buf.len() as u32,
                    &mut read,
                );
                if ok == 0 {
                    body.clear();
                    break;
                }
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..read as usize]);
            }
        }
        InternetCloseHandle(request);
        InternetCloseHandle(session);
        if status_ok && !body.is_empty() {
            Ok(body)
        } else {
            Err(())
        }
    }
}

/// `MinecraftFetcher` 的 WinINet 实现，NbsApp::new 里注入。
pub struct WinMinecraftFetcher;

impl MinecraftFetcher for WinMinecraftFetcher {
    fn fetch_text(&self, url: &str) -> Result<String, ()> {
        fetch_bytes(url).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    // 音效文件是 ogg 等二进制，必须按字节落盘，不能过 from_utf8_lossy。
    fn fetch_file(&self, url: &str, dst: &Path) -> Result<(), ()> {
        let bytes = fetch_bytes(url)?;
        let mut f = std::fs::File::create(dst).map_err(|_| ())?;
        f.write_all(&bytes).map_err(|_| ())?;
        Ok(())
    }
}
