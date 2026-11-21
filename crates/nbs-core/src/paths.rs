//! 「配置放在哪个目录」全仓只有一份答案。
//!
//! 为什么住在最底层的 nbs-core：编辑器把外部控制的发现文件（`control.json`）写到这个目录，
//! MCP 桥又得自己去把它找出来。这条规则原先在 `nbs-mcp` 里手抄了一份——两边结果一致，
//! 但那是**一致**而不是**同源**：这里改了顺序或加了新的兜底分支，桥不会跟着动，
//! 症状是「填好了 IDE 配置却连不上」，而两边各自都答得头头是道。
//!
//! 不放 nbs-io 是因为桥接进程不愿拖进它的全部依赖（`image`、`flate2`、winapi）；
//! 不放 nbs-control 是因为协议层不该管文件落在哪儿。nbs-core 谁都不依赖，谁都能依赖。

use std::path::PathBuf;

/// 配置目录名：`%APPDATA%\nbs-rust`、`~/.config/nbs-rust`。
pub const APP_DIR: &str = "nbs-rust";

/// 一个标准目录都没认出来时的兜底名。带前导点是原规则（编辑器和桥两边一直如此），
/// 和 [`APP_DIR`] 故意不同名——别在这里"顺手统一"，那会让已在跑的桥和编辑器找不着对方的文件。
const FALLBACK_DIR: &str = ".nbs-rust";

/// 本进程的配置文件目录。
pub fn config_dir() -> PathBuf {
    config_dir_from(|key| std::env::var(key).ok())
}

/// 同一条规则，但环境变量从外面喂进来——五层顺序（定向目录 → APPDATA → XDG → HOME → 相对兜底）
/// 是能被逐条走一遍的，而不是只在真实机器上撞一次。
///
/// 语义与 `std::env::var(key).ok()` 严格一致：**只要变量存在**就算，空串也算
/// （空 `APPDATA` 会落到相对路径 `nbs-rust`，不是跳过它去试 XDG）。这条区别留给真实环境里
/// 把变量显式设空的人，别在这里顺手改成「空就当作没给」。
pub fn config_dir_from(mut env: impl FnMut(&str) -> Option<String>) -> PathBuf {
    if let Some(dir) = env("NBS_RUST_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(appdata) = env("APPDATA") {
        return PathBuf::from(appdata).join(APP_DIR);
    }
    if let Some(xdg) = env("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join(APP_DIR);
    }
    if let Some(home) = env("HOME") {
        return PathBuf::from(home).join(".config").join(APP_DIR);
    }
    PathBuf::from(".").join(FALLBACK_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只给了表里写的键，其余当作没设。
    fn env_of(pairs: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn explicit_override_wins_over_every_standard_dir() {
        let dir = config_dir_from(env_of(&[
            ("NBS_RUST_CONFIG_DIR", "/tmp/pointed"),
            ("APPDATA", "/should/not/reach"),
            ("XDG_CONFIG_HOME", "/should/not/reach"),
            ("HOME", "/should/not/reach"),
        ]));
        assert_eq!(dir, PathBuf::from("/tmp/pointed"));
    }

    #[test]
    fn the_three_standard_dirs_are_tried_in_order() {
        assert_eq!(
            config_dir_from(env_of(&[("APPDATA", "C:/Users/me/AppData/Roaming")])),
            PathBuf::from("C:/Users/me/AppData/Roaming").join(APP_DIR)
        );
        assert_eq!(
            config_dir_from(env_of(&[("XDG_CONFIG_HOME", "/home/me/.xdg")])),
            PathBuf::from("/home/me/.xdg").join(APP_DIR)
        );
        assert_eq!(
            config_dir_from(env_of(&[("HOME", "/home/me")])),
            PathBuf::from("/home/me/.config").join(APP_DIR)
        );
    }

    #[test]
    fn appdata_beats_xdg_when_both_are_set() {
        // Windows 上两者可能同时存在（WSL/Git Bash 里 XDG 会漏进来），顺序就是口径。
        let dir = config_dir_from(env_of(&[
            ("APPDATA", "C:/Users/me/AppData/Roaming"),
            ("XDG_CONFIG_HOME", "/home/me/.config"),
        ]));
        assert_eq!(dir, PathBuf::from("C:/Users/me/AppData/Roaming").join(APP_DIR));
    }

    #[test]
    fn nothing_set_falls_back_to_a_relative_dir() {
        assert_eq!(
            config_dir_from(env_of(&[])),
            PathBuf::from(".").join(FALLBACK_DIR)
        );
    }

    #[test]
    fn the_fallback_name_is_deliberately_not_the_app_dir_name() {
        // 兜底那份带前导点，标准目录那份不带。这条差异是从原规则继承来的，
        // 两边（编辑器和桥）现在共用一个函数，所以钉在这里：谁"顺手统一"谁改不到对方的文件。
        assert_ne!(APP_DIR, FALLBACK_DIR);
        assert!(FALLBACK_DIR.starts_with('.'));
        assert!(!APP_DIR.starts_with('.'));
    }

    #[test]
    fn an_empty_variable_counts_as_set() {
        // 与 `std::env::var(k).ok()` 对齐：空串是 Some("")，不会被跳过。
        let dir = config_dir_from(env_of(&[("APPDATA", ""), ("HOME", "/home/me")]));
        assert_eq!(dir, PathBuf::from("").join(APP_DIR));
        assert_ne!(dir, PathBuf::from("/home/me/.config").join(APP_DIR));
    }
}
