use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use aion_ast::{NodeKind, Program, PropValue};
use aion_runtime_net::generate_proto3;
use aion_runtime_ui::{compute_layout, SignalStore, Viewport};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IosBuildOptions {
    pub bundle_id: String,
    pub app_name: String,
    pub version_name: String,
    pub build_number: u32,
    pub api_base_url: Option<String>,
    pub out_dir: PathBuf,
    pub debug: bool,
    pub simulator: bool,
    pub ipa: bool,
    pub no_codesign: bool,
    pub team_id: Option<String>,
    pub export_method: String,
    pub generate_only: bool,
}

impl Default for IosBuildOptions {
    fn default() -> Self {
        Self {
            bundle_id: "com.aion.app".to_string(),
            app_name: "AION App".to_string(),
            version_name: "0.1.0".to_string(),
            build_number: 1,
            api_base_url: None,
            out_dir: PathBuf::from("build/ios"),
            debug: false,
            simulator: false,
            ipa: false,
            no_codesign: false,
            team_id: None,
            export_method: "app-store-connect".to_string(),
            generate_only: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IosBuildResult {
    pub ok: bool,
    pub bundle_id: String,
    pub app_name: String,
    pub version_name: String,
    pub build_number: u32,
    pub project_dir: String,
    pub xcodeproj_path: String,
    pub bundle_json_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xcarchive_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipa_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_size_bytes: Option<u64>,
    pub mode: String,
    pub signed: bool,
}

/// Generates a self-contained native SwiftUI Xcode project for `program` and compiles
/// either an iOS `.app` (Simulator or Device) or an `.xcarchive` + `.ipa` via `xcodebuild`.
pub fn build_ios_app(
    program: &Program,
    opts: &IosBuildOptions,
) -> Result<IosBuildResult, String> {
    validate_bundle_id(&opts.bundle_id)?;

    let project_dir = &opts.out_dir;
    let src_dir = project_dir.join("AionApp");
    let xcodeproj_dir = project_dir.join("AionApp.xcodeproj");
    let schemes_dir = xcodeproj_dir.join("xcshareddata/xcschemes");

    fs::create_dir_all(&src_dir)
        .map_err(|e| format!("failed to create {}: {e}", src_dir.display()))?;
    fs::create_dir_all(&schemes_dir)
        .map_err(|e| format!("failed to create {}: {e}", schemes_dir.display()))?;

    // 1. Precompute mobile (390x844) Taffy layout + Signals + AST + .proto bundle.
    let signal_store = SignalStore::from_program(program);
    let mut initial_signals = serde_json::Map::new();
    for node in &program.nodes {
        if let NodeKind::Signal(_) = &node.kind {
            if let Some(val) = signal_store.get(&node.id) {
                initial_signals.insert(
                    node.id.as_str().to_string(),
                    serde_json::to_value(val).unwrap_or(serde_json::Value::Null),
                );
            }
        }
    }
    let layout = compute_layout(program, Viewport::MOBILE, Some(&signal_store))
        .map_err(|e| format!("layout error: {e}"))?;
    let proto = generate_proto3(program);
    let default_port = extract_port(program).unwrap_or(8080);
    let api_base_url = opts
        .api_base_url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{default_port}"));

    let bundle_payload = json!({
        "apiBaseUrl": api_base_url,
        "program": program,
        "signals": initial_signals,
        "layout": layout,
        "proto": proto,
    });
    let bundle_json_path = src_dir.join("aion_bundle.json");
    fs::write(
        &bundle_json_path,
        serde_json::to_string_pretty(&bundle_payload).unwrap(),
    )
    .map_err(|e| format!("write aion_bundle.json: {e}"))?;

    // 2. Write native SwiftUI runtime, Info.plist, project.pbxproj, and shared scheme.
    fs::write(src_dir.join("AionApp.swift"), generate_swiftui_app())
        .map_err(|e| format!("write AionApp.swift: {e}"))?;
    fs::write(src_dir.join("Info.plist"), generate_info_plist(opts))
        .map_err(|e| format!("write Info.plist: {e}"))?;
    fs::write(
        xcodeproj_dir.join("project.pbxproj"),
        generate_pbxproj(opts),
    )
    .map_err(|e| format!("write project.pbxproj: {e}"))?;
    fs::write(
        schemes_dir.join("AionApp.xcscheme"),
        generate_xcscheme(),
    )
    .map_err(|e| format!("write AionApp.xcscheme: {e}"))?;

    let signed = opts.team_id.is_some() && !opts.no_codesign && !opts.simulator;
    let config = if opts.debug { "Debug" } else { "Release" };

    if opts.generate_only {
        return Ok(IosBuildResult {
            ok: true,
            bundle_id: opts.bundle_id.clone(),
            app_name: opts.app_name.clone(),
            version_name: opts.version_name.clone(),
            build_number: opts.build_number,
            project_dir: project_dir.display().to_string(),
            xcodeproj_path: xcodeproj_dir.display().to_string(),
            bundle_json_path: bundle_json_path.display().to_string(),
            app_path: None,
            xcarchive_path: None,
            ipa_path: None,
            artifact_size_bytes: None,
            mode: "generate-only".to_string(),
            signed,
        });
    }

    // 3. Verify xcodebuild is available (macOS + Xcode).
    let xcodebuild_bin = detect_xcodebuild().ok_or_else(|| {
        "xcodebuild not found. iOS builds require macOS with Xcode installed (`xcode-select --install` / Xcode.app)."
            .to_string()
    })?;

    let abs_project_dir = fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.clone());
    let abs_xcodeproj = abs_project_dir.join("AionApp.xcodeproj");
    let derived_data = abs_project_dir.join("DerivedData");

    if opts.ipa {
        let archive_path = abs_project_dir.join("build/AionApp.xcarchive");
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create archive dir: {e}"))?;
        }
        if archive_path.exists() {
            let _ = fs::remove_dir_all(&archive_path);
        }

        let mut cmd = Command::new(&xcodebuild_bin);
        cmd.arg("-project")
            .arg(&abs_xcodeproj)
            .arg("-scheme")
            .arg("AionApp")
            .arg("-configuration")
            .arg(config)
            .arg("-sdk")
            .arg("iphoneos")
            .arg("-destination")
            .arg("generic/platform=iOS")
            .arg("-derivedDataPath")
            .arg(&derived_data)
            .arg("-archivePath")
            .arg(&archive_path)
            .arg("archive");

        if !signed {
            cmd.arg("CODE_SIGNING_ALLOWED=NO")
                .arg("CODE_SIGNING_REQUIRED=NO")
                .arg("CODE_SIGN_IDENTITY=");
        } else if let Some(ref team) = opts.team_id {
            cmd.arg(format!("DEVELOPMENT_TEAM={team}"))
                .arg("-allowProvisioningUpdates");
        }

        let output = cmd
            .output()
            .map_err(|e| format!("failed to run xcodebuild archive: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(format!(
                "xcodebuild archive failed (exit {:?}):\n{stdout}\n{stderr}",
                output.status.code()
            ));
        }

        let ipa_dir = abs_project_dir.join("build/ipa");
        fs::create_dir_all(&ipa_dir).map_err(|e| format!("create ipa dir: {e}"))?;

        let ipa_path = if signed {
            let export_plist = abs_project_dir.join("build/ExportOptions.plist");
            fs::write(&export_plist, generate_export_options_plist(opts))
                .map_err(|e| format!("write ExportOptions.plist: {e}"))?;
            let exp_out = Command::new(&xcodebuild_bin)
                .arg("-exportArchive")
                .arg("-archivePath")
                .arg(&archive_path)
                .arg("-exportOptionsPlist")
                .arg(&export_plist)
                .arg("-exportPath")
                .arg(&ipa_dir)
                .arg("-allowProvisioningUpdates")
                .output()
                .map_err(|e| format!("failed to run xcodebuild -exportArchive: {e}"))?;
            if !exp_out.status.success() {
                let stderr = String::from_utf8_lossy(&exp_out.stderr);
                let stdout = String::from_utf8_lossy(&exp_out.stdout);
                return Err(format!(
                    "xcodebuild -exportArchive failed (exit {:?}):\n{stdout}\n{stderr}",
                    exp_out.status.code()
                ));
            }
            ipa_dir.join("AionApp.ipa")
        } else {
            // Package unsigned Payload/AionApp.app -> AionApp.ipa
            let archived_app = archive_path.join("Products/Applications/AionApp.app");
            if !archived_app.is_dir() {
                return Err(format!(
                    "xcarchive succeeded but AionApp.app not found at {}",
                    archived_app.display()
                ));
            }
            let stage_dir = abs_project_dir.join("build/ipa_stage");
            let payload_dir = stage_dir.join("Payload");
            if stage_dir.exists() {
                let _ = fs::remove_dir_all(&stage_dir);
            }
            fs::create_dir_all(&payload_dir)
                .map_err(|e| format!("create Payload dir: {e}"))?;
            let dst_app = payload_dir.join("AionApp.app");
            copy_dir_recursive(&archived_app, &dst_app)?;

            let out_ipa = ipa_dir.join("AionApp.ipa");
            if out_ipa.exists() {
                let _ = fs::remove_file(&out_ipa);
            }
            let zip_out = Command::new("/usr/bin/zip")
                .current_dir(&stage_dir)
                .arg("-r")
                .arg("-y")
                .arg("-q")
                .arg(&out_ipa)
                .arg("Payload")
                .output()
                .map_err(|e| format!("failed to run /usr/bin/zip: {e}"))?;
            if !zip_out.status.success() {
                let stderr = String::from_utf8_lossy(&zip_out.stderr);
                return Err(format!("zip failed packaging .ipa: {stderr}"));
            }
            out_ipa
        };

        let size_bytes = fs::metadata(&ipa_path).map(|m| m.len()).ok();
        let mode = format!(
            "ipa-{}-{}",
            config.to_ascii_lowercase(),
            if signed { "signed" } else { "unsigned" }
        );

        return Ok(IosBuildResult {
            ok: true,
            bundle_id: opts.bundle_id.clone(),
            app_name: opts.app_name.clone(),
            version_name: opts.version_name.clone(),
            build_number: opts.build_number,
            project_dir: project_dir.display().to_string(),
            xcodeproj_path: xcodeproj_dir.display().to_string(),
            bundle_json_path: bundle_json_path.display().to_string(),
            app_path: Some(
                archive_path
                    .join("Products/Applications/AionApp.app")
                    .display()
                    .to_string(),
            ),
            xcarchive_path: Some(archive_path.display().to_string()),
            ipa_path: Some(ipa_path.display().to_string()),
            artifact_size_bytes: size_bytes,
            mode,
            signed,
        });
    }

    // Standard `aion build ios`: compile .app for Simulator or Device.
    let (sdk, destination, products_sub) = if opts.simulator {
        (
            "iphonesimulator",
            "generic/platform=iOS Simulator",
            format!("{config}-iphonesimulator"),
        )
    } else {
        (
            "iphoneos",
            "generic/platform=iOS",
            format!("{config}-iphoneos"),
        )
    };

    let mut cmd = Command::new(&xcodebuild_bin);
    cmd.arg("-project")
        .arg(&abs_xcodeproj)
        .arg("-scheme")
        .arg("AionApp")
        .arg("-configuration")
        .arg(config)
        .arg("-sdk")
        .arg(sdk)
        .arg("-destination")
        .arg(destination)
        .arg("-derivedDataPath")
        .arg(&derived_data)
        .arg("build");

    if !signed {
        cmd.arg("CODE_SIGNING_ALLOWED=NO")
            .arg("CODE_SIGNING_REQUIRED=NO")
            .arg("CODE_SIGN_IDENTITY=");
    } else if let Some(ref team) = opts.team_id {
        cmd.arg(format!("DEVELOPMENT_TEAM={team}"))
            .arg("-allowProvisioningUpdates");
    }

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run xcodebuild build: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "xcodebuild build failed (exit {:?}):\n{stdout}\n{stderr}",
            output.status.code()
        ));
    }

    let app_path = derived_data
        .join("Build/Products")
        .join(&products_sub)
        .join("AionApp.app");
    if !app_path.is_dir() {
        return Err(format!(
            "xcodebuild succeeded but AionApp.app not found at {}",
            app_path.display()
        ));
    }

    let binary_size = fs::metadata(app_path.join("AionApp"))
        .map(|m| m.len())
        .ok();
    let mode = format!(
        "ios-{}-{}",
        if opts.simulator { "simulator" } else { "device" },
        config.to_ascii_lowercase()
    );

    Ok(IosBuildResult {
        ok: true,
        bundle_id: opts.bundle_id.clone(),
        app_name: opts.app_name.clone(),
        version_name: opts.version_name.clone(),
        build_number: opts.build_number,
        project_dir: project_dir.display().to_string(),
        xcodeproj_path: xcodeproj_dir.display().to_string(),
        bundle_json_path: bundle_json_path.display().to_string(),
        app_path: Some(app_path.display().to_string()),
        xcarchive_path: None,
        ipa_path: None,
        artifact_size_bytes: binary_size,
        mode,
        signed,
    })
}

fn validate_bundle_id(bundle_id: &str) -> Result<(), String> {
    let parts: Vec<&str> = bundle_id.split('.').collect();
    if parts.len() < 2
        || parts.iter().any(|p| {
            p.is_empty()
                || !p.chars().next().unwrap().is_ascii_alphabetic()
                || !p
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
    {
        return Err(format!(
            "invalid iOS bundle identifier `{bundle_id}` (expected e.g. `com.aion.myapp`)"
        ));
    }
    Ok(())
}

fn extract_port(program: &Program) -> Option<u16> {
    for node in &program.nodes {
        if let NodeKind::Srv(srv) = &node.kind {
            for prop in &srv.props {
                if prop.key == "port" {
                    if let PropValue::Int(p) = prop.value {
                        return Some(p as u16);
                    }
                }
            }
        }
    }
    None
}

fn detect_xcodebuild() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("/usr/bin/xcodebuild"),
        PathBuf::from("/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild"),
    ];
    for c in candidates {
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("entry: {e}"))?;
        let ty = entry.file_type().map_err(|e| format!("file_type: {e}"))?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), &dst_path)
                .map_err(|e| format!("copy {} -> {}: {e}", entry.path().display(), dst_path.display()))?;
        }
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn generate_info_plist(opts: &IosBuildOptions) -> String {
    let display_name = xml_escape(&opts.app_name);
    let version = xml_escape(&opts.version_name);
    let build = opts.build_number;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>{display_name}</string>
	<key>CFBundleExecutable</key>
	<string>$(EXECUTABLE_NAME)</string>
	<key>CFBundleIdentifier</key>
	<string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{display_name}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{build}</string>
	<key>LSRequiresIPhoneOS</key>
	<true/>
	<key>NSAppTransportSecurity</key>
	<dict>
		<key>NSAllowsArbitraryLoads</key>
		<true/>
	</dict>
	<key>UIApplicationSceneManifest</key>
	<dict>
		<key>UIApplicationSupportsMultipleScenes</key>
		<false/>
	</dict>
	<key>UILaunchScreen</key>
	<dict/>
	<key>UISupportedInterfaceOrientations</key>
	<array>
		<string>UIInterfaceOrientationPortrait</string>
		<string>UIInterfaceOrientationLandscapeLeft</string>
		<string>UIInterfaceOrientationLandscapeRight</string>
	</array>
</dict>
</plist>
"#
    )
}

fn generate_export_options_plist(opts: &IosBuildOptions) -> String {
    let method = xml_escape(&opts.export_method);
    let team = opts.team_id.as_deref().unwrap_or("");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>method</key>
	<string>{method}</string>
	<key>signingStyle</key>
	<string>automatic</string>
	<key>teamID</key>
	<string>{team}</string>
	<key>stripSwiftSymbols</key>
	<true/>
	<key>compileBitcode</key>
	<false/>
</dict>
</plist>
"#
    )
}

fn generate_xcscheme() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<Scheme
   LastUpgradeVersion = "1600"
   version = "1.7">
   <BuildAction
      parallelizeBuildables = "YES"
      buildImplicitDependencies = "YES">
      <BuildActionEntries>
         <BuildActionEntry
            buildForTesting = "YES"
            buildForRunning = "YES"
            buildForProfiling = "YES"
            buildForArchiving = "YES"
            buildForAnalyzing = "YES">
            <BuildableReference
               BuildableIdentifier = "primary"
               BlueprintIdentifier = "A10000000000000000000050"
               BuildableName = "AionApp.app"
               BlueprintName = "AionApp"
               ReferencedContainer = "container:AionApp.xcodeproj">
            </BuildableReference>
         </BuildActionEntry>
      </BuildActionEntries>
   </BuildAction>
   <LaunchAction
      buildConfiguration = "Debug"
      selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB"
      selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB"
      launchStyle = "0"
      useCustomWorkingDirectory = "NO"
      ignoresPersistentStateOnLaunch = "NO"
      debugDocumentVersioning = "YES"
      allowLocationSimulation = "YES">
      <BuildableProductRunnable
         runnableDebuggingMode = "0">
         <BuildableReference
            BuildableIdentifier = "primary"
            BlueprintIdentifier = "A10000000000000000000050"
            BuildableName = "AionApp.app"
            BlueprintName = "AionApp"
            ReferencedContainer = "container:AionApp.xcodeproj">
         </BuildableReference>
      </BuildableProductRunnable>
   </LaunchAction>
   <ArchiveAction
      buildConfiguration = "Release"
      revealArchiveInOrganizer = "YES">
   </ArchiveAction>
</Scheme>
"#
    .to_string()
}

fn generate_pbxproj(opts: &IosBuildOptions) -> String {
    let team_setting = opts
        .team_id
        .as_deref()
        .map(|t| format!("\t\t\t\tDEVELOPMENT_TEAM = \"{t}\";\n"))
        .unwrap_or_default();

    format!(
        r#"// !$*UTF8*$!
{{
	archiveVersion = 1;
	classes = {{
	}};
	objectVersion = 56;
	objects = {{

/* Begin PBXBuildFile section */
		A10000000000000000000001 /* AionApp.swift in Sources */ = {{isa = PBXBuildFile; fileRef = A10000000000000000000011 /* AionApp.swift */; }};
		A10000000000000000000002 /* aion_bundle.json in Resources */ = {{isa = PBXBuildFile; fileRef = A10000000000000000000012 /* aion_bundle.json */; }};
/* End PBXBuildFile section */

/* Begin PBXFileReference section */
		A10000000000000000000010 /* AionApp.app */ = {{isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = AionApp.app; sourceTree = BUILT_PRODUCTS_DIR; }};
		A10000000000000000000011 /* AionApp.swift */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = AionApp.swift; sourceTree = "<group>"; }};
		A10000000000000000000012 /* aion_bundle.json */ = {{isa = PBXFileReference; lastKnownFileType = text.json; path = aion_bundle.json; sourceTree = "<group>"; }};
		A10000000000000000000013 /* Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; }};
/* End PBXFileReference section */

/* Begin PBXFrameworksBuildPhase section */
		A10000000000000000000020 /* Frameworks */ = {{
			isa = PBXFrameworksBuildPhase;
			buildActionMask = 2147483647;
			files = (
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
/* End PBXFrameworksBuildPhase section */

/* Begin PBXGroup section */
		A10000000000000000000030 = {{
			isa = PBXGroup;
			children = (
				A10000000000000000000031 /* AionApp */,
				A10000000000000000000032 /* Products */,
			);
			sourceTree = "<group>";
		}};
		A10000000000000000000031 /* AionApp */ = {{
			isa = PBXGroup;
			children = (
				A10000000000000000000011 /* AionApp.swift */,
				A10000000000000000000012 /* aion_bundle.json */,
				A10000000000000000000013 /* Info.plist */,
			);
			path = AionApp;
			sourceTree = "<group>";
		}};
		A10000000000000000000032 /* Products */ = {{
			isa = PBXGroup;
			children = (
				A10000000000000000000010 /* AionApp.app */,
			);
			name = Products;
			sourceTree = "<group>";
		}};
/* End PBXGroup section */

/* Begin PBXNativeTarget section */
		A10000000000000000000050 /* AionApp */ = {{
			isa = PBXNativeTarget;
			buildConfigurationList = A10000000000000000000060 /* Build configuration list for PBXNativeTarget "AionApp" */;
			buildPhases = (
				A10000000000000000000040 /* Sources */,
				A10000000000000000000020 /* Frameworks */,
				A10000000000000000000041 /* Resources */,
			);
			buildRules = (
			);
			dependencies = (
			);
			name = AionApp;
			productName = AionApp;
			productReference = A10000000000000000000010 /* AionApp.app */;
			productType = "com.apple.product-type.application";
		}};
/* End PBXNativeTarget section */

/* Begin PBXProject section */
		A10000000000000000000090 /* Project object */ = {{
			isa = PBXProject;
			attributes = {{
				BuildIndependentTargetsInParallel = 1;
				LastSwiftUpdateCheck = 1600;
				LastUpgradeCheck = 1600;
			}};
			buildConfigurationList = A10000000000000000000070 /* Build configuration list for PBXProject "AionApp" */;
			compatibilityVersion = "Xcode 14.0";
			developmentRegion = en;
			hasScannedForEncodings = 0;
			knownRegions = (
				en,
				Base,
			);
			mainGroup = A10000000000000000000030;
			productRefGroup = A10000000000000000000032 /* Products */;
			projectDirPath = "";
			projectRoot = "";
			targets = (
				A10000000000000000000050 /* AionApp */,
			);
		}};
/* End PBXProject section */

/* Begin PBXResourcesBuildPhase section */
		A10000000000000000000041 /* Resources */ = {{
			isa = PBXResourcesBuildPhase;
			buildActionMask = 2147483647;
			files = (
				A10000000000000000000002 /* aion_bundle.json in Resources */,
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
/* End PBXResourcesBuildPhase section */

/* Begin PBXSourcesBuildPhase section */
		A10000000000000000000040 /* Sources */ = {{
			isa = PBXSourcesBuildPhase;
			buildActionMask = 2147483647;
			files = (
				A10000000000000000000001 /* AionApp.swift in Sources */,
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
/* End PBXSourcesBuildPhase section */

/* Begin XCBuildConfiguration section */
		A10000000000000000000061 /* Debug */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				CODE_SIGN_STYLE = Automatic;
				CURRENT_PROJECT_VERSION = "{build_number}";
{team_setting}				GENERATE_INFOPLIST_FILE = NO;
				INFOPLIST_FILE = AionApp/Info.plist;
				IPHONEOS_DEPLOYMENT_TARGET = 16.0;
				LD_RUNPATH_SEARCH_PATHS = (
					"$(inherited)",
					"@executable_path/Frameworks",
				);
				MARKETING_VERSION = "{version_name}";
				PRODUCT_BUNDLE_IDENTIFIER = "{bundle_id}";
				PRODUCT_NAME = AionApp;
				SDKROOT = iphoneos;
				SWIFT_OPTIMIZATION_LEVEL = "-Onone";
				SWIFT_VERSION = 5.0;
				TARGETED_DEVICE_FAMILY = "1,2";
			}};
			name = Debug;
		}};
		A10000000000000000000062 /* Release */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				CODE_SIGN_STYLE = Automatic;
				CURRENT_PROJECT_VERSION = "{build_number}";
{team_setting}				GENERATE_INFOPLIST_FILE = NO;
				INFOPLIST_FILE = AionApp/Info.plist;
				IPHONEOS_DEPLOYMENT_TARGET = 16.0;
				LD_RUNPATH_SEARCH_PATHS = (
					"$(inherited)",
					"@executable_path/Frameworks",
				);
				MARKETING_VERSION = "{version_name}";
				PRODUCT_BUNDLE_IDENTIFIER = "{bundle_id}";
				PRODUCT_NAME = AionApp;
				SDKROOT = iphoneos;
				SWIFT_OPTIMIZATION_LEVEL = "-O";
				SWIFT_VERSION = 5.0;
				TARGETED_DEVICE_FAMILY = "1,2";
			}};
			name = Release;
		}};
		A10000000000000000000071 /* Debug */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				ALWAYS_SEARCH_USER_PATHS = NO;
				ENABLE_STRICT_OBJC_MSGSEND = YES;
				IPHONEOS_DEPLOYMENT_TARGET = 16.0;
				SDKROOT = iphoneos;
				SWIFT_VERSION = 5.0;
			}};
			name = Debug;
		}};
		A10000000000000000000072 /* Release */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				ALWAYS_SEARCH_USER_PATHS = NO;
				ENABLE_STRICT_OBJC_MSGSEND = YES;
				IPHONEOS_DEPLOYMENT_TARGET = 16.0;
				SDKROOT = iphoneos;
				SWIFT_VERSION = 5.0;
				VALIDATE_PRODUCT = YES;
			}};
			name = Release;
		}};
/* End XCBuildConfiguration section */

/* Begin XCConfigurationList section */
		A10000000000000000000060 /* Build configuration list for PBXNativeTarget "AionApp" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				A10000000000000000000061 /* Debug */,
				A10000000000000000000062 /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
		A10000000000000000000070 /* Build configuration list for PBXProject "AionApp" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				A10000000000000000000071 /* Debug */,
				A10000000000000000000072 /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
/* End XCConfigurationList section */
	}};
	rootObject = A10000000000000000000090 /* Project object */;
}}
"#,
        bundle_id = opts.bundle_id,
        version_name = opts.version_name,
        build_number = opts.build_number,
        team_setting = team_setting,
    )
}

fn generate_swiftui_app() -> &'static str {
    r##"import SwiftUI
import Foundation

/// Native iOS SwiftUI runtime generated by AION.
/// Renders flat #N nodes using precomputed Taffy coordinates, manages #S reactive signals,
/// supports ROUTER + @tap->NAV(#N...) multi-screen switching, and dispatches @tap->CALL(#API...).
@main
struct AionApplication: App {
    @StateObject private var store = AionStore()

    var body: some Scene {
        WindowGroup {
            AionStageView(store: store)
        }
    }
}

struct AionBox {
    let x: CGFloat
    let y: CGFloat
    let width: CGFloat
    let height: CGFloat
    let parent: String?
}

final class AionStore: ObservableObject {
    @Published var signals: [String: Any] = [:]
    @Published var activeScreens: [String: String] = [:]
    @Published var toastMessage: String? = nil
    @Published var loadError: String? = nil

    var persistedSignals: Set<String> = []
    var apiBaseUrl: String = "http://127.0.0.1:8080"
    var viewportWidth: CGFloat = 390
    var viewportHeight: CGFloat = 844
    var paintOrder: [String] = []
    var boxes: [String: AionBox] = [:]
    var nodeMap: [String: [String: Any]] = [:]
    var parentMap: [String: String] = [:]
    var rootBgHex: String = "#F8FAFC"

    init() {
        loadBundle()
    }

    private func loadBundle() {
        guard let url = Bundle.main.url(forResource: "aion_bundle", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            loadError = "Failed to load aion_bundle.json from app bundle."
            return
        }

        if let base = root["apiBaseUrl"] as? String {
            apiBaseUrl = base
        }
        if let sigs = root["signals"] as? [String: Any] {
            signals = sigs
        }

        if let program = root["program"] as? [String: Any],
           let nodes = program["nodes"] as? [[String: Any]] {
            for n in nodes {
                guard let nid = n["id"] as? String,
                      let kind = n["kind"] as? [String: Any] else { continue }
                nodeMap[nid] = kind
                let nt = kind["node_type"] as? String ?? ""
                if nt == "signal", (kind["persist"] as? Bool) == true {
                    persistedSignals.insert(nid)
                    if let savedStr = UserDefaults.standard.string(forKey: "aion_persist:" + nid),
                       let savedData = savedStr.data(using: .utf8),
                       let savedVal = try? JSONSerialization.jsonObject(with: savedData, options: .fragmentsAllowed) {
                        signals[nid] = savedVal
                    }
                } else if nt == "ui" {
                    let widget = kind["widget"] as? String ?? ""
                    let args = kind["args"] as? [[String: Any]] ?? []
                    if widget == "ROOT" {
                        rootBgHex = argString(args, 4, "#F8FAFC")
                    } else if widget == "ROUTER" {
                        var initial = argStringOpt(args, 0)
                        if initial == nil,
                           let body = kind["body"] as? [String: Any],
                           (body["body_kind"] as? String) == "children",
                           let ch = body["value"] as? [String],
                           let first = ch.first {
                            initial = first
                        }
                        if let initScreen = initial {
                            activeScreens[nid] = initScreen
                        }
                    }
                }
            }
        }

        if let layout = root["layout"] as? [String: Any] {
            if let vp = layout["viewport"] as? [String: Any] {
                viewportWidth = CGFloat((vp["width"] as? NSNumber)?.doubleValue ?? 390)
                viewportHeight = CGFloat((vp["height"] as? NSNumber)?.doubleValue ?? 844)
            }
            if let po = layout["paint_order"] as? [String] {
                paintOrder = po
            }
            if let rawBoxes = layout["boxes"] as? [String: [String: Any]] {
                for (id, b) in rawBoxes {
                    let p = b["parent"] as? String
                    let box = AionBox(
                        x: CGFloat((b["x"] as? NSNumber)?.doubleValue ?? 0),
                        y: CGFloat((b["y"] as? NSNumber)?.doubleValue ?? 0),
                        width: max(1, CGFloat((b["width"] as? NSNumber)?.doubleValue ?? 1)),
                        height: max(1, CGFloat((b["height"] as? NSNumber)?.doubleValue ?? 1)),
                        parent: p
                    )
                    boxes[id] = box
                    if let p = p, !p.isEmpty {
                        parentMap[id] = p
                    }
                }
            }
        }

        for (_, kind) in nodeMap where (kind["node_type"] as? String) == "ui" {
            runNodeEvent(kind, eventName: "init", item: nil, index: nil)
        }
    }

    func savePersistedSignals() {
        for id in persistedSignals {
            guard let val = signals[id] else { continue }
            if let data = try? JSONSerialization.data(withJSONObject: val, options: .fragmentsAllowed),
               let str = String(data: data, encoding: .utf8) {
                UserDefaults.standard.set(str, forKey: "aion_persist:" + id)
            }
        }
    }

    func dynamicListSignal(_ ui: [String: Any]) -> String? {
        guard (ui["widget"] as? String) == "LIST",
              let args = ui["args"] as? [[String: Any]] else { return nil }
        for a in args where (a["arg_kind"] as? String) == "node_ref" {
            return a["value"] as? String
        }
        return nil
    }

    func isNodeVisible(_ id: String) -> Bool {
        var cur: String? = id
        while let c = cur, let p = parentMap[c] {
            if let pUi = nodeMap[p] {
                if dynamicListSignal(pUi) != nil {
                    return false
                }
                if (pUi["widget"] as? String) == "ROUTER",
                   let active = activeScreens[p], active != c {
                    return false
                }
            }
            cur = p
        }
        return true
    }

    func navigateTo(_ targetId: String) {
        for (routerId, ui) in nodeMap {
            guard (ui["node_type"] as? String) == "ui",
                  (ui["widget"] as? String) == "ROUTER",
                  let body = ui["body"] as? [String: Any],
                  (body["body_kind"] as? String) == "children",
                  let children = body["value"] as? [String] else { continue }
            if children.contains(targetId) {
                activeScreens[routerId] = targetId
                if let screenUi = nodeMap[targetId] {
                    runNodeEvent(screenUi, eventName: "init", item: nil, index: nil)
                }
                return
            }
        }
    }

    func resolveText(_ template: String, item: Any? = nil) -> String {
        var out = template
        for (k, v) in signals {
            let token = "{\(k)}"
            if out.contains(token) {
                out = out.replacingOccurrences(of: token, with: stringifyValue(v))
            }
        }
        if let dict = item as? [String: Any] {
            for (fk, fv) in dict {
                out = out.replacingOccurrences(of: "{item.\(fk)}", with: stringifyValue(fv))
            }
        } else if let item = item {
            out = out.replacingOccurrences(of: "{item}", with: stringifyValue(item))
        }
        return out
    }

    func signalString(_ id: String) -> String {
        guard let v = signals[id] else { return "" }
        return stringifyValue(v)
    }

    func setSignal(_ id: String, value: Any) {
        signals[id] = value
        savePersistedSignals()
    }

    func handleNodeClick(_ ui: [String: Any]) {
        runNodeEvent(ui, eventName: "tap", item: nil, index: nil)
    }

    func runNodeEvent(_ ui: [String: Any], eventName: String, item: Any?, index: Int?) {
        guard let events = ui["events"] as? [[String: Any]],
              let handler = events.first(where: { ($0["event"] as? String) == eventName }),
              let act = handler["action"] as? [String: Any] else { return }

        let actKind = act["action_kind"] as? String ?? ""
        if actKind == "generic" {
            let name = act["name"] as? String ?? ""
            let actArgs = act["args"] as? [[String: Any]] ?? []
            if name == "NAV", let first = actArgs.first,
               let kind = first["kind"] as? [String: Any],
               let target = kind["value"] as? String {
                navigateTo(target)
                return
            }
            if name == "PUSH", actArgs.count >= 2,
               let firstKind = actArgs[0]["kind"] as? [String: Any],
               let sigId = firstKind["value"] as? String {
                var cur = (signals[sigId] as? [Any]) ?? []
                cur.append(evalExpr(actArgs[1], res: nil, item: item, index: index))
                signals[sigId] = cur
                savePersistedSignals()
                return
            }
            if name == "REMOVE", actArgs.count >= 2,
               let firstKind = actArgs[0]["kind"] as? [String: Any],
               let sigId = firstKind["value"] as? String {
                var cur = (signals[sigId] as? [Any]) ?? []
                let targetVal = evalExpr(actArgs[1], res: nil, item: item, index: index)
                if let idx = (targetVal as? NSNumber)?.intValue, idx >= 0, idx < cur.count {
                    cur.remove(at: idx)
                } else if let i = index, i >= 0, i < cur.count {
                    cur.remove(at: i)
                }
                signals[sigId] = cur
                savePersistedSignals()
                return
            }
            return
        }
        if actKind == "set", let sigId = act["signal"] as? String {
            signals[sigId] = evalExpr(act["value"] as? [String: Any], res: nil, item: item, index: index)
            savePersistedSignals()
            return
        }
        if actKind == "toast" {
            showToast(stringifyValue(evalExpr(act["message"] as? [String: Any], res: nil, item: item, index: index)))
            return
        }

        guard actKind == "call",
              let apiId = act["api"] as? String,
              let apiNode = nodeMap[apiId] else { return }

        let method = (apiNode["method"] as? String ?? "POST").uppercased()
        let path = apiNode["path"] as? String ?? "/"
        let payloadVal = evalExpr(act["payload"] as? [String: Any], res: nil, item: item, index: index)
        let base = apiBaseUrl.hasSuffix("/") ? String(apiBaseUrl.dropLast()) : apiBaseUrl
        guard let url = URL(string: base + path) else {
            triggerErr(events: events, fallback: "Invalid URL")
            return
        }

        var req = URLRequest(url: url)
        req.httpMethod = method
        req.timeoutInterval = 8
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if method != "GET",
           let dict = payloadVal as? [String: Any],
           let bodyData = try? JSONSerialization.data(withJSONObject: dict) {
            req.httpBody = bodyData
        }

        let bindDict = act["bind"] as? [String: Any]
        URLSession.shared.dataTask(with: req) { [weak self] data, response, error in
            DispatchQueue.main.async {
                guard let self = self else { return }
                if let error = error {
                    self.triggerErr(events: events, fallback: error.localizedDescription)
                    return
                }
                let status = (response as? HTTPURLResponse)?.statusCode ?? 500
                if status < 200 || status >= 300 {
                    self.triggerErr(events: events, fallback: "HTTP \(status)")
                    return
                }
                var parsedRes: Any = [:]
                if let data = data, let json = try? JSONSerialization.jsonObject(with: data) {
                    parsedRes = json
                }
                if let bind = bindDict,
                   let target = bind["signal"] as? String,
                   let expr = bind["value"] as? [String: Any] {
                    self.signals[target] = self.evalExpr(expr, res: parsedRes, item: item, index: index)
                    self.savePersistedSignals()
                }
            }
        }.resume()
    }

    private func triggerErr(events: [[String: Any]], fallback: String) {
        if let errEv = events.first(where: { ($0["event"] as? String) == "err" }),
           let act = errEv["action"] as? [String: Any],
           (act["action_kind"] as? String) == "toast",
           let msgExpr = act["message"] as? [String: Any] {
            showToast(stringifyValue(evalExpr(msgExpr, res: nil, item: nil, index: nil)))
        } else {
            showToast(fallback)
        }
    }

    private func showToast(_ msg: String) {
        toastMessage = msg
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { [weak self] in
            if self?.toastMessage == msg {
                self?.toastMessage = nil
            }
        }
    }

    private func evalExpr(_ expr: [String: Any]?, res: Any?, item: Any?, index: Int?) -> Any {
        guard let expr = expr, let kind = expr["kind"] as? [String: Any] else { return "" }
        let ek = kind["expr_kind"] as? String ?? ""
        switch ek {
        case "int", "float", "str", "bool":
            return kind["value"] ?? ""
        case "node_ref":
            if let refId = kind["value"] as? String {
                return signals[refId] ?? ""
            }
            return ""
        case "ident":
            let name = kind["value"] as? String ?? ""
            if name == "res" { return res ?? [:] }
            if name == "item" { return item ?? [:] }
            if name == "index" { return index ?? 0 }
            return name
        case "member":
            if let val = kind["value"] as? [String: Any],
               let obj = evalExpr(val["object"] as? [String: Any], res: res, item: item, index: index) as? [String: Any],
               let field = val["field"] as? String {
                return obj[field] ?? ""
            }
            return ""
        case "object":
            var out: [String: Any] = [:]
            if let pairs = kind["value"] as? [[Any]] {
                for pair in pairs where pair.count == 2 {
                    if let k = pair[0] as? String, let v = pair[1] as? [String: Any] {
                        out[k] = evalExpr(v, res: res, item: item, index: index)
                    }
                }
            }
            return out
        default:
            return ""
        }
    }

    func argFloat(_ args: [[String: Any]], _ idx: Int, _ def: CGFloat) -> CGFloat {
        guard idx < args.count else { return def }
        let a = args[idx]
        if let n = a["value"] as? NSNumber { return CGFloat(n.doubleValue) }
        return def
    }

    func argString(_ args: [[String: Any]], _ idx: Int, _ def: String) -> String {
        argStringOpt(args, idx) ?? def
    }

    func argStringOpt(_ args: [[String: Any]], _ idx: Int) -> String? {
        guard idx < args.count else { return nil }
        return args[idx]["value"] as? String
    }

    private func stringifyValue(_ v: Any) -> String {
        if let s = v as? String { return s }
        if let b = v as? Bool { return b ? "true" : "false" }
        if let n = v as? NSNumber { return n.stringValue }
        return "\(v)"
    }
}

struct AionStageView: View {
    @ObservedObject var store: AionStore

    var body: some View {
        ZStack {
            Color(aionHex: store.rootBgHex)
                .ignoresSafeArea()

            if let err = store.loadError {
                Text(err)
                    .foregroundColor(.red)
                    .padding()
            } else {
                GeometryReader { geo in
                    let scaleX = geo.size.width / max(1, store.viewportWidth)
                    let scaleY = geo.size.height / max(1, store.viewportHeight)
                    let scale = min(scaleX, scaleY)
                    let offsetX = (geo.size.width - store.viewportWidth * scale) / 2
                    let offsetY = (geo.size.height - store.viewportHeight * scale) / 2

                    ZStack(alignment: .topLeading) {
                        ForEach(store.paintOrder, id: \.self) { id in
                            if store.isNodeVisible(id),
                               let box = store.boxes[id],
                               let ui = store.nodeMap[id],
                               (ui["node_type"] as? String) == "ui" {
                                AionNodeView(store: store, ui: ui, scale: scale)
                                    .frame(width: box.width * scale, height: box.height * scale)
                                    .position(
                                        x: offsetX + (box.x + box.width / 2) * scale,
                                        y: offsetY + (box.y + box.height / 2) * scale
                                    )
                            }
                        }
                    }
                }
            }

            if let toast = store.toastMessage {
                VStack {
                    Spacer()
                    Text(toast)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(.white)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        .background(Color.black.opacity(0.85))
                        .cornerRadius(10)
                        .padding(.bottom, 28)
                }
                .transition(.opacity)
            }
        }
    }
}

struct AionNodeView: View {
    @ObservedObject var store: AionStore
    let ui: [String: Any]
    let scale: CGFloat

    var body: some View {
        let widget = ui["widget"] as? String ?? ""
        let args = ui["args"] as? [[String: Any]] ?? []
        let bodyDict = ui["body"] as? [String: Any]

        switch widget {
        case "ROOT":
            Color(aionHex: store.argString(args, 4, "#F8FAFC"))
        case "NAV":
            Color(aionHex: store.argString(args, 2, "#0F172A"))
        case "BTN":
            let radius = store.argFloat(args, 2, 8) * scale
            let bg = store.argString(args, 3, "#2563EB")
            Button(action: { store.handleNodeClick(ui) }) {
                RoundedRectangle(cornerRadius: radius)
                    .fill(Color(aionHex: bg))
            }
            .buttonStyle(.plain)
        case "INP":
            let radius = store.argFloat(args, 2, 8) * scale
            let bg = store.argString(args, 3, "#FFFFFF")
            let boundSig = store.argStringOpt(args, 4)
            let placeholder = ((bodyDict?["value"] as? [String: Any])?["template"] as? String) ?? ""
            TextField(
                placeholder,
                text: Binding(
                    get: { boundSig.map { store.signalString($0) } ?? "" },
                    set: { newVal in
                        if let sig = boundSig {
                            store.setSignal(sig, value: newVal)
                        }
                    }
                )
            )
            .font(.system(size: 14 * scale))
            .foregroundColor(Color(aionHex: "#0F172A"))
            .padding(.horizontal, 12 * scale)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(
                RoundedRectangle(cornerRadius: radius)
                    .fill(Color(aionHex: bg))
                    .overlay(
                        RoundedRectangle(cornerRadius: radius)
                            .stroke(Color(aionHex: "#CBD5E1"), lineWidth: 1)
                    )
            )
        case "TXT":
            let fontSize = store.argFloat(args, 0, 16) * scale
            let weightVal = Int(store.argFloat(args, 1, 400))
            let colorHex = store.argString(args, 2, "#0F172A")
            let tpl = ((bodyDict?["value"] as? [String: Any])?["template"] as? String) ?? ""
            Text(store.resolveText(tpl))
                .font(.system(size: fontSize, weight: weightVal >= 600 ? .bold : .regular))
                .foregroundColor(Color(aionHex: colorHex))
                .lineLimit(1)
                .allowsHitTesting(false)
        case "LIST":
            if let sigId = store.dynamicListSignal(ui),
               let items = store.signals[sigId] as? [Any],
               let children = bodyDict?["value"] as? [String],
               let tplId = children.first {
                ScrollView {
                    VStack(spacing: store.argFloat(args, 1, 8) * scale) {
                        ForEach(Array(items.enumerated()), id: \.offset) { idx, elem in
                            AionDynamicRowView(store: store, tplId: tplId, item: elem, index: idx, scale: scale)
                        }
                    }
                }
            } else {
                Color.clear
            }
        default:
            Color.clear
        }
    }
}

struct AionDynamicRowView: View {
    @ObservedObject var store: AionStore
    let tplId: String
    let item: Any
    let index: Int
    let scale: CGFloat

    var body: some View {
        let ui = store.nodeMap[tplId] ?? [:]
        let widget = ui["widget"] as? String ?? ""
        let args = ui["args"] as? [[String: Any]] ?? []
        let bodyDict = ui["body"] as? [String: Any]
        let children = (bodyDict?["value"] as? [String]) ?? []

        if widget == "TXT" {
            let fs = store.argFloat(args, 0, 14) * scale
            let w = Int(store.argFloat(args, 1, 400))
            let hex = store.argString(args, 2, "#0F172A")
            let tpl = ((bodyDict?["value"] as? [String: Any])?["template"] as? String) ?? ""
            Text(store.resolveText(tpl, item: item))
                .font(.system(size: fs, weight: w >= 600 ? .bold : .regular))
                .foregroundColor(Color(aionHex: hex))
        } else if widget == "BTN" {
            let bw = store.argFloat(args, 0, 80) * scale
            let bh = store.argFloat(args, 1, 32) * scale
            let r = store.argFloat(args, 2, 6) * scale
            let bg = store.argString(args, 3, "#2563EB")
            Button(action: { store.runNodeEvent(ui, eventName: "tap", item: item, index: index) }) {
                ZStack {
                    RoundedRectangle(cornerRadius: r).fill(Color(aionHex: bg))
                    if let first = children.first {
                        AionDynamicRowView(store: store, tplId: first, item: item, index: index, scale: scale)
                    }
                }
                .frame(width: bw, height: bh)
            }
            .buttonStyle(.plain)
        } else {
            HStack {
                ForEach(Array(children.enumerated()), id: \.offset) { idx, chId in
                    AionDynamicRowView(store: store, tplId: chId, item: item, index: index, scale: scale)
                    if idx == 0 && children.count > 1 { Spacer() }
                }
            }
            .padding(.horizontal, 10 * scale)
            .padding(.vertical, 8 * scale)
            .background(RoundedRectangle(cornerRadius: 8 * scale).fill(Color.white))
        }
    }
}

extension Color {
    init(aionHex hex: String) {
        let cleaned = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: cleaned).scanHexInt64(&int)
        let r, g, b, a: UInt64
        switch cleaned.count {
        case 6:
            (r, g, b, a) = ((int >> 16) & 0xFF, (int >> 8) & 0xFF, int & 0xFF, 255)
        case 8:
            (r, g, b, a) = ((int >> 24) & 0xFF, (int >> 16) & 0xFF, (int >> 8) & 0xFF, int & 0xFF)
        default:
            (r, g, b, a) = (15, 23, 42, 255)
        }
        self.init(
            .sRGB,
            red: Double(r) / 255.0,
            green: Double(g) / 255.0,
            blue: Double(b) / 255.0,
            opacity: Double(a) / 255.0
        )
    }
}
"##
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_parser::parse_program;

    #[test]
    fn generates_valid_ios_xcode_project_with_router_and_bundle() {
        let src = r#"
            #SRV0:HTTP[port:8080, proto:on]
            #S0:i32=0
            #N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
            #N1:FLEX[v,s,st,0]{#N2,#N3}
            #N2:NAV[100%,56,#0F172A]{#N4,#N5}
            #N4:BTN[100,40,8,#2563EB]{#N6} @tap->NAV(#N7)
            #N5:BTN[100,40,8,#334155]{#N8} @tap->NAV(#N9)
            #N6:TXT[14,600,#FFFFFF]{"Home"}
            #N8:TXT[14,600,#FFFFFF]{"Cards"}
            #N3:ROUTER[#N7]{#N7,#N9}
            #N7:FLEX[v,c,c,12]{#N10}
            #N9:FLEX[v,c,c,12]{#N11}
            #N10:TXT[20,700,#0F172A]{"Budget {#S0}"}
            #N11:TXT[20,700,#0F172A]{"Cards Screen"}
        "#;
        let prog = parse_program(src).expect("parse");
        let tmp = std::env::temp_dir().join("aion_ios_unit_test");
        let _ = fs::remove_dir_all(&tmp);

        let opts = IosBuildOptions {
            bundle_id: "com.aion.budget".to_string(),
            app_name: "Budget AION".to_string(),
            version_name: "1.0.0".to_string(),
            build_number: 3,
            out_dir: tmp.clone(),
            generate_only: true,
            ..Default::default()
        };

        let res = build_ios_app(&prog, &opts).expect("generate ios project");
        assert!(res.ok);
        assert_eq!(res.mode, "generate-only");
        assert!(tmp.join("AionApp.xcodeproj/project.pbxproj").is_file());
        assert!(tmp
            .join("AionApp.xcodeproj/xcshareddata/xcschemes/AionApp.xcscheme")
            .is_file());
        assert!(tmp.join("AionApp/AionApp.swift").is_file());
        assert!(tmp.join("AionApp/Info.plist").is_file());
        assert!(tmp.join("AionApp/aion_bundle.json").is_file());

        let pbx = fs::read_to_string(tmp.join("AionApp.xcodeproj/project.pbxproj")).unwrap();
        assert!(pbx.contains("PRODUCT_BUNDLE_IDENTIFIER = \"com.aion.budget\""));
        assert!(pbx.contains("CURRENT_PROJECT_VERSION = \"3\""));

        let swift = fs::read_to_string(tmp.join("AionApp/AionApp.swift")).unwrap();
        assert!(swift.contains("navigateTo"));
        assert!(swift.contains("ROUTER"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
