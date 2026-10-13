//! Compare supplied canonical paths without performing filesystem IO on the UI thread.
use fastdiff_cli::OpenRequest;

pub(super) fn same_repository(
    left: &OpenRequest,
    right: &OpenRequest,
    native_windows: bool,
) -> bool {
    if native_windows && left.wsl.is_none() && right.wsl.is_none() {
        return windows_path_key(&left.repository) == windows_path_key(&right.repository);
    }
    left.repository == right.repository
}

fn windows_path_key(path: &str) -> String {
    let path = path.replace('/', "\\");
    let mut normalized = if path
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        format!(r"\\{}", &path[8..])
    } else if let Some(drive_path) = path.strip_prefix(r"\\?\")
        && has_drive(drive_path)
    {
        drive_path.to_owned()
    } else {
        path
    };
    if has_drive(&normalized) {
        normalized.replace_range(..1, &normalized[..1].to_ascii_lowercase());
    }
    // Preserve a drive root's separator so absolute C:\ stays distinct from drive-relative C:.
    while normalized.ends_with('\\')
        && !(has_drive(&normalized) && normalized.len() == 3)
        && normalized.len() > 2
    {
        normalized.pop();
    }
    normalized
}

fn has_drive(path: &str) -> bool {
    path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && path.as_bytes().get(1) == Some(&b':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::Fastdiff, benchmark::Benchmark, state::Preferences};
    use fastdiff_cli::WslTarget;
    use fastdiff_core::View;

    fn request(path: &str) -> OpenRequest {
        OpenRequest {
            target: fastdiff_cli::OpenTarget::Repository,
            repository: path.into(),
            file: None,
            view: View::All,
            wsl: None,
            theme: false,
        }
    }

    #[test]
    fn windows_drive_prefix_separators_and_drive_case_identify_one_repository() {
        assert!(same_repository(
            &request(r"C:\Users\Dev\repository"),
            &request(r"\\?\C:\Users\Dev\repository"),
            true
        ));
        assert!(same_repository(
            &request("C:/Users/Dev/repository/"),
            &request(r"c:\Users\Dev\repository"),
            true
        ));
        assert!(!same_repository(
            &request(r"C:\repository"),
            &request(r"C:\another"),
            true
        ));
        assert!(same_repository(
            &request(r"C:\"),
            &request(r"\\?\c:\"),
            true
        ));
        assert!(!same_repository(&request(r"C:\"), &request("C:"), true));
    }

    #[test]
    fn windows_unc_prefix_identifies_the_same_network_repository() {
        assert!(same_repository(
            &request(r"\\server\share\repository"),
            &request(r"\\?\UNC\server\share\repository"),
            true
        ));
        assert!(same_repository(
            &request("//server/share/repository/"),
            &request(r"\\?\unc\server\share\repository"),
            true
        ));
    }

    #[test]
    fn linux_paths_are_not_normalized_even_when_they_look_like_windows_paths() {
        assert!(!same_repository(
            &request(r"C:\repository"),
            &request("c:/repository"),
            false
        ));
    }

    #[test]
    fn windows_gui_preserves_wsl_linux_case_and_literal_backslashes() {
        let wsl = WslTarget {
            distribution: "Ubuntu".into(),
            user: "dev".into(),
            helper: "/home/dev/fastdiff".into(),
        };
        let mut upper = request("/home/dev/Repository");
        upper.wsl = Some(wsl.clone());
        let mut lower = request("/home/dev/repository");
        lower.wsl = Some(wsl.clone());
        assert!(!same_repository(&upper, &lower, true));
        let mut slash = request("/home/dev/repo/name");
        slash.wsl = Some(wsl.clone());
        let mut backslash = request(r"/home/dev/repo\name");
        backslash.wsl = Some(wsl);
        assert!(!same_repository(&slash, &backslash, true));
        assert!(same_repository(&backslash, &backslash, true));
    }

    #[test]
    fn tab_identity_keeps_wsl_distribution_user_and_helper_boundaries() {
        let mut tab = crate::app::tests::fixture();
        tab.request.repository = "/home/dev/repository".into();
        tab.request.wsl = Some(WslTarget {
            distribution: "Ubuntu".into(),
            user: "dev".into(),
            helper: "/home/dev/fastdiff".into(),
        });
        let forwarded = tab.request.clone();
        let viewer = Fastdiff {
            tabs: vec![tab],
            selected_tab: 0,
            preferences: Preferences::default(),
            server: None,
            benchmark: Benchmark::new(false),
            help_open: false,
            retired_workers: Vec::new(),
        };
        assert_eq!(viewer.matching_tab(&forwarded, true), Some(0));
        let mut distribution = forwarded.clone();
        distribution.wsl.as_mut().unwrap().distribution = "Debian".into();
        assert_eq!(viewer.matching_tab(&distribution, true), None);
        let mut user = forwarded.clone();
        user.wsl.as_mut().unwrap().user = "other".into();
        assert_eq!(viewer.matching_tab(&user, true), None);
        let mut helper = forwarded.clone();
        helper.wsl.as_mut().unwrap().helper = "/home/dev/new-helper".into();
        assert_eq!(viewer.matching_tab(&helper, true), None);
    }
}
