//! The app's XPC services in `Contents/XPCServices`: each a program of the app's package in a bundle of its own,
//! signed before the app with entitlements of its own (E22, as Dango bundles them).

use std::path::{Path, PathBuf};

/// A helper program the app starts as an XPC service, from `Contents/XPCServices/<name>.xpc`.
pub(crate) struct Service {
    /// The bundle's name.
    pub(crate) name: &'static str,
    /// The program of the app's package that the service runs.
    pub(crate) program: &'static str,
    /// The entitlements it is signed with, each set to true.
    pub(crate) entitlements: &'static [&'static str],
}

/// The app's XPC services. The decoder reads untrusted pictures and runs in the App Sandbox, which the app itself
/// never does, as its shells would inherit it.
pub(crate) const SERVICES: &[Service] = &[Service {
    // splat_decoder looks it up as the app's identifier with `.decoder`.
    name: "Decoder",
    program: "term-decoder",
    entitlements: &["com.apple.security.app-sandbox"],
}];

const PLIST_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
"#;

impl Service {
    pub(crate) fn bundle(&self, app: &Path) -> PathBuf {
        app.join("Contents/XPCServices").join(format!("{}.xpc", self.name))
    }

    pub(crate) fn executable(&self, app: &Path) -> PathBuf {
        self.bundle(app).join("Contents/MacOS").join(self.program)
    }

    /// The app's identifier with the service's name, so that each build reaches its own service.
    pub(crate) fn bundle_id(&self, app_id: &str) -> String {
        format!("{app_id}.{}", self.name.to_lowercase())
    }

    pub(crate) fn info_plist(&self, app_id: &str, version: &str, minimum_system: &str) -> String {
        let (name, program, bundle_id) = (self.name, self.program, self.bundle_id(app_id));
        format!(
            r#"{PLIST_HEAD}<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>{program}</string>
	<key>CFBundleIdentifier</key>
	<string>{bundle_id}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>CFBundlePackageType</key>
	<string>XPC!</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>{minimum_system}</string>
	<key>XPCService</key>
	<dict>
		<key>ServiceType</key>
		<string>Application</string>
	</dict>
</dict>
</plist>
"#
        )
    }

    /// The service's entitlements as a property list.
    pub(crate) fn entitlements_plist(&self) -> String {
        let keys: String = self
            .entitlements
            .iter()
            .map(|key| format!("\t<key>{key}</key>\n\t<true/>\n"))
            .collect();
        format!("{PLIST_HEAD}<dict>\n{keys}</dict>\n</plist>\n")
    }

    /// Writes the entitlements into `target/` for codesign; their file.
    pub(crate) fn write_entitlements(&self) -> Result<PathBuf, String> {
        let file = crate::target_dir().join(format!("{}.entitlements", self.program));
        std::fs::write(&file, self.entitlements_plist()).map_err(|e| format!("{}: {e}", file.display()))?;
        Ok(file)
    }

    /// Lays the service's bundle out inside `app` around `program`, the built program.
    pub(crate) fn assemble(
        &self,
        app: &Path,
        program: &Path,
        app_id: &str,
        version: &str,
        minimum_system: &str,
    ) -> Result<(), String> {
        let executable = self.executable(app);
        let folder = executable.parent().ok_or("no MacOS folder")?;
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        std::fs::copy(program, &executable).map_err(|e| format!("{}: {e}", program.display()))?;
        let plist = self.bundle(app).join("Contents/Info.plist");
        std::fs::write(&plist, self.info_plist(app_id, version, minimum_system)).map_err(|e| e.to_string())?;
        crate::run(std::process::Command::new("plutil").args(["-lint", "-s"]).arg(&plist))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_decoder_is_a_sandboxed_service_named_after_the_app_without_the_product() {
        let app = Path::new("/x/A.app");
        let service = &SERVICES[0];
        assert_eq!(
            service.executable(app),
            Path::new("/x/A.app/Contents/XPCServices/Decoder.xpc/Contents/MacOS/term-decoder")
        );
        assert_eq!(service.bundle_id("local.x.debug"), "local.x.debug.decoder");
        let plist = service.info_plist(term_product::BUNDLE_ID, "1.2.3", "14.0");
        assert!(plist.contains("<string>XPC!</string>"));
        assert!(!plist.to_lowercase().contains(&term_product::NAME.to_lowercase()));
        assert!(service.entitlements_plist().contains("com.apple.security.app-sandbox"));
    }
}
