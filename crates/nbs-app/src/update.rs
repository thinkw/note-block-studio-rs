//! 开机更新检查：原版 create 事件里 `http_get(link_releases / link_latest)`
//! （control_create.gml:593-616），HTTP 回调 `check_updates.gml` 解析 GitHub API
//! 的 JSON。GameMaker 底层就是 WinINet，这里走同一条路；结果写进共享状态，
//! UI 线程每帧 `poll_update_check`。状态值沿用原版：-1 失败 / 0 检查中 /
//! 1 有新版本 / 2 已最新。

#[cfg(windows)]
use nbs_editor::state::SharedUpdateCheck;

/// 与 lib.rs 的 RELEASES_PAGE 同一个仓库，只换成 GitHub API 域名。
const API_BASE: &str = "https://api.github.com/repos/thinkw/note-block-studio-rs";

/// 原版在 create 事件发起请求，这里起后台线程（不阻塞窗口启动）。
#[cfg(windows)]
pub fn spawn(check_prerelease: bool, current: String, shared: SharedUpdateCheck) {
    std::thread::spawn(move || {
        // check_prerelease → /releases 列表取第一个（不管是不是正式版）；
        // 否则 /releases/latest 单对象。与原版 `release = res[0]` / `release = res` 对应。
        let url = if check_prerelease {
            format!("{API_BASE}/releases")
        } else {
            format!("{API_BASE}/releases/latest")
        };
        let result = match fetch(&url) {
            Ok(body) => resolve(&body, check_prerelease, &current),
            Err(()) => Err(()),
        };
        if let Ok(mut g) = shared.lock() {
            match result {
                Ok((status, ver)) => {
                    g.status = status;
                    g.new_version = ver;
                }
                Err(()) => g.status = -1,
            }
        }
    });
}

/// check_updates.gml 中段的 JSON 部分：取 release.tag_name 去掉 "v" 前缀，
/// 与本机版本比大小。不可解析 → -1；不比当前新 → 2；比当前新 → 1。
fn resolve(body: &str, check_prerelease: bool, current: &str) -> Result<(i8, String), ()> {
    let json: serde_json::Value = serde_json::from_str(body).map_err(|_| ())?;
    let release = if check_prerelease {
        json.as_array().and_then(|a| a.first()).ok_or(())?
    } else {
        &json
    };
    let tag = release
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or(())?;
    // `if (string_char_at(new_version, 1) == "v") new_version = string_delete(new_version, 1, 1)`
    let new_version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    match compare_versions(&new_version, current) {
        None => Ok((-1, String::new())),
        Some(order) if order <= 0 => Ok((2, String::new())),
        Some(_) => Ok((1, new_version)),
    }
}

/// 原 check_updates.gml 末尾的两个解析/比较函数逐行移植，保持 SemVer 语义：
/// 数字核心段、正式版高于预发布版、预发布数字段按数值比（beta.10 > beta.6）、
/// build 元数据不参与排序。
fn parse_version(value: &str) -> Option<(Vec<f64>, Vec<String>)> {
    let value = value.strip_prefix('v').unwrap_or(value);
    // `build = string_pos("+", value)`；`string_copy(value, 1, build - 1)` 截在它前面。
    let value = match value.find('+') {
        Some(i) => &value[..i],
        None => value,
    };
    let (value, prerelease) = match value.find('-') {
        Some(i) => {
            let pre: Vec<String> = value[i + 1..].split('.').map(str::to_string).collect();
            if pre.iter().any(|s| s.is_empty()) {
                return None;
            }
            (&value[..i], pre)
        }
        None => (value, Vec::new()),
    };
    let core: Vec<&str> = value.split('.').collect();
    if core.len() != 3 {
        return None;
    }
    let mut nums = Vec::with_capacity(3);
    for c in core {
        // `core[i] == "" || string_digits(core[i]) != core[i]` → undefined
        if c.is_empty() || !c.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // GML real() 是双精度，超大数字也按 f64 处理
        nums.push(c.parse::<f64>().ok()?);
    }
    Some((nums, prerelease))
}

fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn is_all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// 原 update_compare_versions → {-1, 0, 1}；任一侧解析失败返回 None。
pub fn compare_versions(candidate: &str, installed: &str) -> Option<i32> {
    let (nc, np) = parse_version(candidate)?;
    let (oc, op) = parse_version(installed)?;
    for i in 0..3 {
        if nc[i] != oc[i] {
            return Some(sign(nc[i] - oc[i]) as i32);
        }
    }
    // 任何一侧没有预发布段：正式版高。两侧都无 → 0。
    if np.is_empty() || op.is_empty() {
        return Some((sign(op.len() as f64) - sign(np.len() as f64)) as i32);
    }
    for i in 0..np.len().min(op.len()) {
        let new_part = &np[i];
        let old_part = &op[i];
        if new_part == old_part {
            continue;
        }
        let new_numeric = is_all_digits(new_part);
        let old_numeric = is_all_digits(old_part);
        if new_numeric && old_numeric {
            let (a, b) = (new_part.parse::<f64>().unwrap(), old_part.parse::<f64>().unwrap());
            return Some(sign(a - b) as i32);
        }
        if new_numeric != old_numeric {
            // SemVer：纯数字段排在字母段之前
            return Some(if new_numeric { -1 } else { 1 });
        }
        return Some(if new_part > old_part { 1 } else { -1 });
    }
    Some(sign(np.len() as f64 - op.len() as f64) as i32)
}

/// GameMaker `http_get` 的底层就是 WinINet，直接同路：状态码 200 才算成功。
#[cfg(windows)]
fn fetch(url: &str) -> Result<String, ()> {
    use std::ffi::CString;
    use winapi::um::wininet::*;

    let agent = CString::new("nbs-rust").map_err(|_| ())?;
    let url = CString::new(url).map_err(|_| ())?;
    // GitHub API 没有 UA 一律 403
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
            let mut buf = [0u8; 4096];
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
            Ok(String::from_utf8_lossy(&body).into_owned())
        } else {
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_basics() {
        let (c, p) = parse_version("1.2.3").unwrap();
        assert_eq!(c, vec![1.0, 2.0, 3.0]);
        assert!(p.is_empty());
        // "v" 前缀剥一次
        assert_eq!(parse_version("v1.2.3"), parse_version("1.2.3"));
        // build 元数据截掉
        let (c, p) = parse_version("1.2.3+build.5").unwrap();
        assert_eq!(c, vec![1.0, 2.0, 3.0]);
        assert!(p.is_empty());
        // 预发布段
        let (c, p) = parse_version("1.2.3-beta.10").unwrap();
        assert_eq!(c, vec![1.0, 2.0, 3.0]);
        assert_eq!(p, vec!["beta".to_string(), "10".to_string()]);
    }

    #[test]
    fn parse_version_rejects() {
        assert_eq!(parse_version("1.2"), None); // 不足三段
        assert_eq!(parse_version("1.2.x"), None); // 非数字段
        assert_eq!(parse_version("1..3"), None); // 空核心段
        assert_eq!(parse_version("1.2.3-"), None); // 空预发布段
        assert_eq!(parse_version("1.2.3-alpha."), None); // 空预发布段
    }

    #[test]
    fn compare_versions_core() {
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Some(0));
        assert_eq!(compare_versions("1.2.4", "1.2.3"), Some(1));
        assert_eq!(compare_versions("1.2.3", "1.2.4"), Some(-1));
        assert_eq!(compare_versions("1.3.0", "1.2.9"), Some(1));
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Some(1));
    }

    #[test]
    fn compare_versions_prerelease_ordering() {
        // beta.10 > beta.6（数值比较，不是字典序）
        assert_eq!(compare_versions("1.3.0-beta.10", "1.3.0-beta.6"), Some(1));
        assert_eq!(compare_versions("1.3.0-beta.6", "1.3.0-beta.10"), Some(-1));
        // 正式版高于预发布版
        assert_eq!(compare_versions("1.3.0", "1.3.0-beta.1"), Some(1));
        assert_eq!(compare_versions("1.3.0-beta.1", "1.3.0"), Some(-1));
        // 同段字典序：rc > beta
        assert_eq!(compare_versions("1.3.0-rc.1", "1.3.0-beta.2"), Some(1));
        // 纯数字段排在字母段之前
        assert_eq!(compare_versions("1.3.0-1", "1.3.0-alpha"), Some(-1));
        assert_eq!(compare_versions("1.3.0-alpha", "1.3.0-1"), Some(1));
        // 多段：前段相等，多出的段决定大小
        assert_eq!(compare_versions("1.3.0-beta.1.2", "1.3.0-beta.1"), Some(1));
        assert_eq!(compare_versions("1.3.0-beta.1", "1.3.0-beta.1.2"), Some(-1));
    }

    #[test]
    fn compare_versions_build_metadata_ignored() {
        assert_eq!(compare_versions("1.2.3+b1", "1.2.3+b2"), Some(0));
    }

    #[test]
    fn compare_versions_invalid() {
        assert_eq!(compare_versions("abc", "1.2.3"), None);
        assert_eq!(compare_versions("1.0", "1.2.3"), None);
        assert_eq!(compare_versions("1.2.3", "xyz"), None);
    }

    #[test]
    fn resolve_stable_uses_object() {
        let (status, ver) = resolve(r#"{"tag_name":"v9.9.9"}"#, false, "0.1.0").unwrap();
        assert_eq!(status, 1);
        assert_eq!(ver, "9.9.9");
    }

    #[test]
    fn resolve_prerelease_uses_first_item() {
        let (status, ver) = resolve(
            r#"[{"tag_name":"v9.9.9-rc.1"},{"tag_name":"v1.0.0"}]"#,
            true,
            "0.1.0",
        )
        .unwrap();
        assert_eq!(status, 1);
        assert_eq!(ver, "9.9.9-rc.1");
    }

    #[test]
    fn resolve_edge_cases() {
        // 空列表 → 原版 release = -1 → update = -1
        assert_eq!(resolve(r#"[]"#, true, "0.1.0"), Err(()));
        // 不比当前新 → 2
        assert_eq!(resolve(r#"{"tag_name":"v0.1.0"}"#, false, "0.1.0"), Ok((2, String::new())));
        // tag 缺失 / tag 无法解析成版本 / JSON 坏 → 原版里这些会让 GML 直接报错，
        // 重写版统一按「检查失败」处理（-1）
        assert_eq!(resolve(r#"{"name":"x"}"#, false, "0.1.0"), Err(()));
        assert_eq!(resolve(r#"{"tag_name":"nope"}"#, false, "0.1.0"), Ok((-1, String::new())));
        assert_eq!(resolve(r#"not json"#, false, "0.1.0"), Err(()));
    }
}
