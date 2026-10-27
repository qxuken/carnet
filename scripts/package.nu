#!/usr/bin/env nu
# Builds Carnet's installers into dist/, from this checkout and the kui
# release Cargo.toml pins, with the version Cargo.toml names:
#
#   nu scripts/package.nu mac       Carnet-<v>-macos.dmg: Carnet.app, for
#                                   Apple silicon (Intel Macs are not built for)
#   nu scripts/package.nu windows   Carnet-<v>-windows-x64-setup.exe, and the
#                                   bare carnet.exe in a .zip beside it
#   nu scripts/package.nu linux     Carnet-<v>-linux-{x64,arm64}.tar.gz: the
#                                   binary, a menu entry, the icon, install.sh
#   nu scripts/package.nu all       the three, from a Mac
#
# Pushing a v* tag runs .github/workflows/release.yml, which runs
# `windows`, `linux` and (on GitHub's macOS runner) `mac`, and puts every
# file on a draft GitHub Release. A file built by hand goes on it with
# `gh release upload v<v> dist/<file> --clobber`.
#
# Needs, for each:
#   mac      a Mac with Xcode's command line tools
#   windows  on a Mac or Linux: cargo-xwin (cargo install cargo-xwin --locked),
#            LLVM's llvm-rc for the exe's icon (brew install llvm) and Docker
#            (NSIS runs in the build image); on Windows: MSVC and NSIS
#   linux    Docker
# The build image (packaging/Dockerfile, which runs packaging/build-env.sh)
# is made on first use and cached. Inside it, or anywhere build-env.sh has
# run with CARNET_BUILD_ENV=1 (CI's job), Linux and NSIS run directly.
#
# Signing. Unsigned, a downloaded Carnet.app is stopped by Gatekeeper and a
# downloaded setup.exe by SmartScreen; both can be let through by hand
# (README.md's Installing says how). To sign the Mac's:
#   $env.CARNET_SIGN_IDENTITY   "Developer ID Application: …" in the keychain
#   $env.CARNET_NOTARY_PROFILE  a `xcrun notarytool store-credentials` profile;
#                               with it the .dmg is notarized and stapled
# Without an identity the app is signed ad hoc, which Apple silicon needs
# to run it at all.

const ROOT = path self ..
const APP = "Carnet"
const BUNDLE_ID = "dev.qxuken.carnet"
const IMAGE = "carnet-build"

# Runs an external command and fails the script when it fails.
def --wrapped must [cmd: string, ...args] {
    ^$cmd ...$args
    if $env.LAST_EXIT_CODE != 0 {
        error make {msg: $"failed \(exit ($env.LAST_EXIT_CODE)\): ($cmd) ($args | str join ' ')"}
    }
}

def step [what: string] { print $"\n=== ($what)" }

def need [tool: string, hint: string] {
    if (which $tool | is-empty) { error make {msg: $"($tool) is not installed: ($hint)"} }
}

def version [] { open ($ROOT | path join Cargo.toml) | get package.version }

def dist [] {
    let d = ($ROOT | path join dist)
    mkdir $d
    $d
}

# Whether this is the build image or CI's job (packaging/build-env.sh's
# environment), where Linux builds and NSIS run without Docker.
def in-build-env [] { ($env.CARNET_BUILD_ENV? | default "") == "1" }

# The Docker image Linux builds and the Windows installer run in.
def build-image [] {
    step "build image"
    must docker build -q -t $IMAGE ($ROOT | path join packaging)
}

def main [] {
    print "nu scripts/package.nu (mac | windows | linux | all); the top of this file says what each needs"
}

# Carnet.app in a .dmg, for Apple silicon.
def "main mac" [] {
    if $nu.os-info.name != "macos" { error make {msg: "the .app and .dmg are made on a Mac"} }
    cd $ROOT
    let v = (version)
    let target = "aarch64-apple-darwin"

    step $"build ($target)"
    with-env {MACOSX_DEPLOYMENT_TARGET: "11.0"} { must cargo build --release --locked --target $target }

    step "Carnet.app"
    let out = (dist)
    let work = ($out | path join mac)
    rm -rf $work
    let app = ($work | path join $"($APP).app")
    let contents = ($app | path join Contents)
    mkdir ($contents | path join MacOS) ($contents | path join Resources)
    let bin = ($contents | path join MacOS carnet)
    cp $"target/($target)/release/carnet" $bin
    cp assets/icon/carnet.icns ($contents | path join Resources carnet.icns)
    open --raw packaging/macos/Info.plist | str replace --all "@VERSION@" $v | save ($contents | path join Info.plist)
    "APPL????" | save --raw ($contents | path join PkgInfo)
    must plutil -lint -s ($contents | path join Info.plist)

    let identity = ($env.CARNET_SIGN_IDENTITY? | default "")
    if ($identity | is-empty) {
        print "signing ad hoc (no CARNET_SIGN_IDENTITY)"
        must codesign --force --sign - --identifier $BUNDLE_ID $app
    } else {
        print $"signing as ($identity)"
        must codesign --force --options runtime --timestamp --sign $identity --identifier $BUNDLE_ID $app
    }
    must codesign --verify --strict $app

    step "the .dmg"
    let stage = ($work | path join dmg)
    mkdir $stage
    cp -r $app $stage
    ^ln -s /Applications ($stage | path join Applications)
    let dmg = ($out | path join $"($APP)-($v)-macos.dmg")
    rm -f $dmg
    must hdiutil create -quiet -volname $"($APP) ($v)" -srcfolder $stage -fs HFS+ -format UDZO -ov $dmg
    if ($identity | is-not-empty) {
        must codesign --force --timestamp --sign $identity $dmg
        let profile = ($env.CARNET_NOTARY_PROFILE? | default "")
        if ($profile | is-not-empty) {
            step "notarize"
            must xcrun notarytool submit $dmg --keychain-profile $profile --wait
            must xcrun stapler staple $dmg
        }
    }
    print $"\n($dmg)"
}

# The NSIS installer and a portable .zip, cross-built off Windows. NSIS
# runs in the build image, unless this is Windows or the image itself.
def "main windows" [] {
    cd $ROOT
    let v = (version)
    let target = "x86_64-pc-windows-msvc"
    let on_windows = ($nu.os-info.name == "windows")
    let nsis_here = ($on_windows or (in-build-env))
    if $nsis_here {
        need makensis "NSIS, from nsis.sourceforge.io"
    } else {
        need docker "Docker Desktop, or docker on Linux"
    }

    step $"build ($target)"
    if $on_windows {
        must cargo build --release --locked --target $target
    } else {
        need cargo-xwin "cargo install cargo-xwin --locked"
        # build.rs compiles the icon with llvm-rc; Homebrew's LLVM is keg-only.
        let llvm = ["/opt/homebrew/opt/llvm/bin" "/usr/local/opt/llvm/bin"] | where {|p| $p | path exists }
        with-env {PATH: ($llvm ++ $env.PATH)} {
            if (which llvm-rc | is-empty) {
                print "warning: no llvm-rc (brew install llvm): carnet.exe will have no icon of its own"
            }
            must cargo xwin build --release --locked --target $target
        }
    }
    let exe = ($ROOT | path join target $target release carnet.exe)

    step "installer"
    let out = (dist)
    let setup_name = $"($APP)-($v)-windows-x64-setup.exe"
    let setup = ($out | path join $setup_name)
    if $nsis_here {
        must makensis -V2 $"-DVERSION=($v)" $"-DEXE=($exe)" $"-DOUT=($setup)" packaging/windows/carnet.nsi
    } else {
        build-image
        (must docker run --rm
            -v $"($ROOT):/src/carnet:ro"
            -v $"($out):/dist"
            $IMAGE
            makensis -V2 $"-DVERSION=($v)" $"-DEXE=/src/carnet/target/($target)/release/carnet.exe"
                $"-DOUT=/dist/($setup_name)" /src/carnet/packaging/windows/carnet.nsi)
    }

    step "portable .zip"
    let zip = ($out | path join $"($APP)-($v)-windows-x64.zip")
    rm -f $zip
    let stage = ($out | path join windows)
    rm -rf $stage
    mkdir $stage
    cp $exe ($stage | path join carnet.exe)
    cd $stage
    must zip -q -X $zip carnet.exe
    cd $ROOT
    rm -rf $stage
    print $"\n($setup)\n($zip)"
}

# Tarballs for x64 and arm64, built against glibc 2.28: in the build
# image, or right here when this is it (CI's job, or the image itself).
def "main linux" [
    --arch: string = "both"  # x64, arm64 or both
] {
    cd $ROOT
    let v = (version)
    let arches = match $arch {
        "both" => [x64 arm64]
        "x64" | "arm64" => [$arch]
        _ => { error make {msg: $"--arch is x64, arm64 or both, not ($arch)"} }
    }
    let triple = {x64: "x86_64-unknown-linux-gnu", arm64: "aarch64-unknown-linux-gnu"}
    let target_dir = ($ROOT | path join target linux)
    mkdir $target_dir

    step $"build ($arches | str join ', ')"
    if (in-build-env) {
        let cargo_target = ($env.CARGO_TARGET_DIR? | default ($ROOT | path join target))
        for a in $arches {
            let t = ($triple | get $a)
            with-env (linux-env $t) { must cargo zigbuild --release --locked --target $"($t).2.28" }
            mkdir ($target_dir | path join $t)
            cp ($cargo_target | path join $t release carnet) ($target_dir | path join $t carnet)
        }
    } else {
        need docker "Docker Desktop, or docker on Linux"
        build-image
        # Carnet's sources read-only. The target dir and cargo's registry
        # are Docker volumes, kept between runs: rustc maps files it writes,
        # which a macOS bind mount answers with SIGBUS. Only the finished
        # binaries come out, into target/linux.
        let builds = ($arches | each {|a|
            let t = ($triple | get $a)
            let vars = (linux-env $t | items {|k, val| $"($k)='($val)'" } | str join " ")
            [
                $"env ($vars) cargo zigbuild --release --locked --target ($t).2.28"
                $"mkdir -p /out/($t) && cp /target/($t)/release/carnet /out/($t)/"
            ]
        } | flatten | str join " && ")
        (must docker run --rm
            -v $"($ROOT):/src/carnet:ro"
            -v $"($target_dir):/out"
            -v carnet-linux-target:/target
            -v carnet-cargo-registry:/usr/local/cargo/registry
            -e CARGO_TARGET_DIR=/target
            -w /src/carnet
            $IMAGE
            sh -euc $builds)
    }

    let out = (dist)
    for a in $arches {
        step $"Carnet-($v)-linux-($a).tar.gz"
        let name = $"($APP)-($v)-linux-($a)"
        let stage = ($out | path join $name)
        rm -rf $stage
        mkdir $stage
        cp ($target_dir | path join ($triple | get $a) carnet) $stage
        cp packaging/linux/carnet.desktop packaging/linux/install.sh $stage
        cp assets/icon/carnet-256.png ($stage | path join carnet.png)
        cp assets/icon/carnet.svg $stage
        let tgz = ($out | path join $"($name).tar.gz")
        must tar -czf $tgz -C $out $name
        rm -rf $stage
        print $tgz
    }
}

# What a Linux target builds with: pkg-config pointed at that
# architecture's ALSA (Debian's multiarch dir is x86_64-linux-gnu, not
# Rust's triple), and the link let through bookworm's libasound naming
# glibc symbols newer than 2.28 - the user's own libasound, built for
# their glibc, is what loads.
def linux-env [triple: string] {
    let pc = $"/usr/lib/($triple | str replace '-unknown-' '-')/pkgconfig"
    let flags_var = $"CARGO_TARGET_($triple | str uppercase | str replace --all '-' '_')_RUSTFLAGS"
    {
        PKG_CONFIG_ALLOW_CROSS: "1"
        PKG_CONFIG_PATH: $pc
        PKG_CONFIG_LIBDIR: $pc
    } | insert $flags_var "-C link-arg=-Wl,--allow-shlib-undefined"
}

# Every installer, from a Mac.
def "main all" [] {
    main mac
    main windows
    main linux
}
