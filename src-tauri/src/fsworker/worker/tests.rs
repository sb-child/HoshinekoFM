#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    // --
    // 模拟 InotifyManager 的 WatchEntry 和路由函数
    // --

    struct WatchEntry {
        is_dir: bool,
        is_dual: bool,
    }

    /// 与 InotifyManager::run 中路由逻辑完全一致。
    fn route_match(p: &PathBuf, t: &PathBuf, e: &WatchEntry) -> bool {
        if e.is_dual {
            p.parent().map_or(false, |par| par == t.as_path()) || p.as_path() == t.as_path()
        } else if e.is_dir {
            p.parent().map_or(false, |par| par == t.as_path())
        } else {
            p.starts_with(t.parent().unwrap_or(t)) || p.as_path() == t.as_path()
        }
    }

    /// 模拟 handle_watch 的核心升级/更新逻辑。
    fn handle_watch_upsert(
        watches: &mut HashMap<PathBuf, WatchEntry>,
        path: &PathBuf,
        is_dir: bool,
    ) {
        if let Some(existing) = watches.get_mut(path) {
            if existing.is_dual {
                existing.is_dual = false;
            }
            if is_dir {
                existing.is_dir = true;
            }
            return;
        }
        watches.insert(
            path.clone(),
            WatchEntry {
                is_dir,
                is_dual: false,
            },
        );
    }

    // --
    // 路由测试
    // --

    #[test]
    fn test_routing_integration() {
        // --
        // 1. 目录 watch (is_dir=true)：以目标自身为前缀
        // --
        let e_dir = WatchEntry {
            is_dir: true,
            is_dual: false,
        };
        // 自身不匹配（防止 phantom 条目）
        let t_home = PathBuf::from("/home");

        // 自身不匹配 ---- 这是本次修复的核心
        assert!(!route_match(&t_home, &t_home, &e_dir), "目录不应匹配自身");
        // 直接子路径匹配
        assert!(route_match(
            &PathBuf::from("/home/sbchild"),
            &t_home,
            &e_dir
        ));
        // 孙路径不匹配（NonRecursive inotify 不产生此类事件）
        assert!(!route_match(
            &PathBuf::from("/home/sbchild/Documents"),
            &t_home,
            &e_dir
        ));

        // 同级目录不能匹配
        assert!(!route_match(&PathBuf::from("/etc"), &t_home, &e_dir));
        // proc 文件不能泄漏进 home
        assert!(!route_match(
            &PathBuf::from("/proc/mounts"),
            &t_home,
            &e_dir
        ));
        assert!(!route_match(
            &PathBuf::from("/proc/uptime"),
            &t_home,
            &e_dir
        ));
        assert!(!route_match(
            &PathBuf::from("/proc/1234/status"),
            &t_home,
            &e_dir
        ));

        // 深层目录同样安全
        let t_etc = PathBuf::from("/etc");
        assert!(!route_match(&PathBuf::from("/home"), &t_etc, &e_dir));
        assert!(!route_match(&PathBuf::from("/usr"), &t_etc, &e_dir));

        // 根目录 watch：只匹配直接子项
        let t_root = PathBuf::from("/");
        assert!(route_match(&PathBuf::from("/home"), &t_root, &e_dir));
        assert!(route_match(&PathBuf::from("/proc"), &t_root, &e_dir));
        assert!(!route_match(&PathBuf::from("/proc/stat"), &t_root, &e_dir));
        assert!(!route_match(&PathBuf::from("/"), &t_root, &e_dir));

        // --
        // 2. 文件 watch (is_dir=false)：以父目录为前缀
        // --
        let e_file = WatchEntry {
            is_dir: false,
            is_dual: false,
        };
        let t_mounts = PathBuf::from("/proc/mounts");

        // 自身匹配
        assert!(route_match(&t_mounts, &t_mounts, &e_file));
        // 同 parent 下的兄弟匹配（notify 对文件用 parent dir 的 inotify watch）
        assert!(route_match(
            &PathBuf::from("/proc/uptime"),
            &t_mounts,
            &e_file
        ));

        // 不相关路径不应匹配
        assert!(!route_match(
            &PathBuf::from("/home/sbchild"),
            &t_mounts,
            &e_file
        ));
        assert!(!route_match(
            &PathBuf::from("/etc/passwd"),
            &t_mounts,
            &e_file
        ));

        // --
        // 3. dual-watch 路由：只匹配自身或直接子项
        // --
        let e_dual = WatchEntry {
            is_dir: true,
            is_dual: true,
        };
        let t_proc = PathBuf::from("/proc");

        // 自身匹配
        assert!(route_match(&t_proc, &t_proc, &e_dual));
        // 直接子项匹配
        assert!(route_match(
            &PathBuf::from("/proc/mounts"),
            &t_proc,
            &e_dual
        ));
        assert!(route_match(
            &PathBuf::from("/proc/uptime"),
            &t_proc,
            &e_dual
        ));
        // 深层不匹配
        assert!(
            !route_match(&PathBuf::from("/proc/1234/status"), &t_proc, &e_dual),
            "dual-watch 不应匹配非直接子项"
        );
        // 无关路径不匹配
        assert!(!route_match(
            &PathBuf::from("/home/sbchild"),
            &t_proc,
            &e_dual
        ));
    }

    // --
    // handle_watch 升级测试
    // --

    #[test]
    fn test_handle_watch_upgrade() {
        let mut watches: HashMap<PathBuf, WatchEntry> = HashMap::new();

        // 场景：/home/sbchild 被 watch，自动创建 /home 的 dual-watch
        watches.insert(
            PathBuf::from("/home/sbchild"),
            WatchEntry {
                is_dir: true,
                is_dual: false,
            },
        );
        watches.insert(
            PathBuf::from("/home"),
            WatchEntry {
                is_dir: true,
                is_dual: true,
            },
        );

        // stat watch 监听 /home -> dual-watch 升级为 primary (is_dir 保持 true)
        handle_watch_upsert(&mut watches, &PathBuf::from("/home"), false);
        {
            let e = watches.get(&PathBuf::from("/home")).unwrap();
            assert!(!e.is_dual, "dual -> primary");
            assert!(e.is_dir, "dual-watch 默认 is_dir=true，不降级");
        }

        // 导航到 /home -> watch_dir -> is_dir 应更新为 true
        handle_watch_upsert(&mut watches, &PathBuf::from("/home"), true);
        {
            let e = watches.get(&PathBuf::from("/home")).unwrap();
            assert!(!e.is_dual, "已是 primary");
            assert!(e.is_dir, "目录 watch 应为 is_dir=true");
        }

        // /proc/mounts breadcrumb -> dual-watch /proc (is_dir=true, is_dual=true)
        watches.insert(
            PathBuf::from("/proc/mounts"),
            WatchEntry {
                is_dir: false,
                is_dual: false,
            },
        );
        watches.insert(
            PathBuf::from("/proc"),
            WatchEntry {
                is_dir: true,
                is_dual: true,
            },
        );

        // 再次 subscribe /proc/mounts -> is_dir 应保持 false, is_dual 升级
        handle_watch_upsert(&mut watches, &PathBuf::from("/proc/mounts"), false);
        {
            let e = watches.get(&PathBuf::from("/proc/mounts")).unwrap();
            assert!(!e.is_dual, "is_dual 应为 false");
            assert!(!e.is_dir, "is_dir 保持 false");
        }
    }
}
