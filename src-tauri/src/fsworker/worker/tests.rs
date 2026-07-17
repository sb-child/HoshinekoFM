#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum WatchScope {
        Children,
        SelfOnly,
    }

    struct WatchEntry {
        scope: WatchScope,
    }

    fn route_match(p: &PathBuf, t: &PathBuf, e: &WatchEntry) -> bool {
        match e.scope {
            WatchScope::Children => p.parent().map_or(false, |par| par == t.as_path()),
            WatchScope::SelfOnly => p.as_path() == t.as_path(),
        }
    }

    fn handle_watch_upsert(
        watches: &mut HashMap<PathBuf, WatchEntry>,
        path: &PathBuf,
        scope: WatchScope,
    ) {
        if let Some(existing) = watches.get_mut(path) {
            if scope == WatchScope::Children {
                existing.scope = WatchScope::Children;
            }
            return;
        }
        watches.insert(path.clone(), WatchEntry { scope });
    }

    #[test]
    fn test_routing_integration() {
        let e_children = WatchEntry {
            scope: WatchScope::Children,
        };
        let t_home = PathBuf::from("/home");

        assert!(!route_match(&t_home, &t_home, &e_children), "目录自身不匹配");
        assert!(route_match(
            &PathBuf::from("/home/sbchild"),
            &t_home,
            &e_children
        ));
        assert!(!route_match(
            &PathBuf::from("/home/sbchild/Documents"),
            &t_home,
            &e_children
        ));
        assert!(!route_match(&PathBuf::from("/etc"), &t_home, &e_children));
        assert!(!route_match(
            &PathBuf::from("/proc/mounts"),
            &t_home,
            &e_children
        ));

        let t_root = PathBuf::from("/");
        assert!(route_match(&PathBuf::from("/home"), &t_root, &e_children));
        assert!(route_match(&PathBuf::from("/proc"), &t_root, &e_children));
        assert!(!route_match(&PathBuf::from("/proc/stat"), &t_root, &e_children));
        assert!(!route_match(&PathBuf::from("/"), &t_root, &e_children));

        let e_self = WatchEntry {
            scope: WatchScope::SelfOnly,
        };
        let t_mounts = PathBuf::from("/proc/mounts");

        assert!(route_match(&t_mounts, &t_mounts, &e_self));
        assert!(!route_match(
            &PathBuf::from("/proc/uptime"),
            &t_mounts,
            &e_self
        ));
        assert!(!route_match(
            &PathBuf::from("/proc/124/status"),
            &t_mounts,
            &e_self
        ));
        assert!(!route_match(
            &PathBuf::from("/home/sbchild"),
            &t_mounts,
            &e_self
        ));
    }

    #[test]
    fn test_self_only_no_cross_dir_leak() {
        let e_self = WatchEntry {
            scope: WatchScope::SelfOnly,
        };
        let t_proc = PathBuf::from("/proc");

        assert!(route_match(&t_proc, &t_proc, &e_self));
        assert!(!route_match(&PathBuf::from("/proc/124"), &t_proc, &e_self));
        assert!(!route_match(
            &PathBuf::from("/proc/124/cmdline"),
            &t_proc,
            &e_self
        ));
    }

    #[test]
    fn test_scope_upgrade_only() {
        let mut watches: HashMap<PathBuf, WatchEntry> = HashMap::new();

        handle_watch_upsert(&mut watches, &PathBuf::from("/proc"), WatchScope::SelfOnly);
        assert_eq!(
            watches.get(&PathBuf::from("/proc")).unwrap().scope,
            WatchScope::SelfOnly
        );

        handle_watch_upsert(&mut watches, &PathBuf::from("/proc"), WatchScope::Children);
        assert_eq!(
            watches.get(&PathBuf::from("/proc")).unwrap().scope,
            WatchScope::Children
        );

        handle_watch_upsert(&mut watches, &PathBuf::from("/proc"), WatchScope::SelfOnly);
        assert_eq!(
            watches.get(&PathBuf::from("/proc")).unwrap().scope,
            WatchScope::Children,
            "SelfOnly 不降级 Children"
        );
    }
}
