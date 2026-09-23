# Install the Oxide CLI.
#
#   irm https://github.com/jaysonwu991/oxide/releases/latest/download/install.ps1 | iex
#
# Environment overrides:
#   OXIDE_VERSION      version to install, with or without a leading "v"
#                      (default: latest release)
#   OXIDE_INSTALL_DIR  directory to install the binary into
#                      (default: %LOCALAPPDATA%\Programs\Oxide on Windows,
#                       $HOME/.local/bin elsewhere)
#   OXIDE_REPO         GitHub repository slug (default: jaysonwu991/oxide)

& {
    $ErrorActionPreference = "Stop"

    $Repo = if ($env:OXIDE_REPO) { $env:OXIDE_REPO } else { "jaysonwu991/oxide" }
    $Version = if ($env:OXIDE_VERSION) { $env:OXIDE_VERSION } else { "" }
    $BaseUrl = "https://github.com/$Repo"
    $ManifestName = "Oxide-manifest"

    function Write-Info([string]$Message) {
        Write-Host "oxide-install: $Message"
    }

    function Write-Warn([string]$Message) {
        Write-Warning "oxide-install: $Message"
    }

    function Get-Os {
        try {
            if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)) {
                return "windows"
            }
            if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::OSX)) {
                return "darwin"
            }
            if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Linux)) {
                return "linux"
            }
        } catch {
        }
        if ($env:OS -eq "Windows_NT") { return "windows" }
        throw "unsupported operating system"
    }

    function Get-Arch {
        try {
            $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
        } catch {
            $arch = $env:PROCESSOR_ARCHITECTURE
        }
        switch -Regex ($arch) {
            "^(X64|AMD64)$" { return "x64" }
            "^(Arm64|ARM64)$" { return "arm64" }
            default { throw "unsupported architecture: $arch" }
        }
    }

    function Get-Platform {
        $key = "$(Get-Os)-$(Get-Arch)"
        switch ($key) {
            "windows-x64" { return "win32-x64" }
            "darwin-arm64" { return "darwin-arm64" }
            "darwin-x64" { return "darwin-x64" }
            "linux-x64" { return "linux-x64" }
            "linux-arm64" { return "linux-arm64" }
            default { throw "no prebuilt binary available for $key" }
        }
    }

    function Get-Text([string]$Url) {
        $response = Invoke-WebRequest -Uri $Url -UseBasicParsing
        $content = $response.Content
        if ($content -is [byte[]]) {
            return [System.Text.Encoding]::UTF8.GetString($content)
        }
        return [string]$content
    }

    function Get-Binary([string]$Url, [string]$OutFile) {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    }

    function Get-ManifestValue([string]$Manifest, [string]$Key) {
        $pattern = "^\s*" + [regex]::Escape($Key) + "\s*:\s*(.+?)\s*$"
        foreach ($line in ($Manifest -split "`r?`n")) {
            if ($line -match $pattern) { return $Matches[1] }
        }
        return $null
    }

    function Resolve-Url([string]$Platform) {
        $ext = if ($Platform -like "win32-*") { "zip" } else { "tar.gz" }

        if ($Version) {
            $ver = $Version -replace "^v", ""
            return "$BaseUrl/releases/download/v$ver/Oxide-v$ver-$Platform.$ext"
        }

        Write-Info "fetching $ManifestName"
        try {
            $manifest = Get-Text "$BaseUrl/releases/latest/download/$ManifestName"
        } catch {
            throw "could not fetch $ManifestName; has a release been published?"
        }
        $ver = Get-ManifestValue $manifest "version"
        if (-not $ver) { throw "could not read version from $ManifestName" }
        $asset = Get-ManifestValue $manifest $Platform
        if (-not $asset) {
            throw "no asset for $Platform; supported targets: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64"
        }
        return "$BaseUrl/releases/download/$ver/$asset"
    }

    function Test-Checksum([string]$File, [string]$SumsFile) {
        $line = Get-Content -Path $SumsFile -TotalCount 1
        if (-not $line) { return }
        $line = $line -replace "^\\", ""
        $expected = (($line -split "\s+") | Where-Object { $_ })[0]
        if (-not $expected) { return }
        $actual = (Get-FileHash -Path $File -Algorithm SHA256).Hash
        if ($expected.ToLower() -ne $actual.ToLower()) {
            throw "checksum verification failed"
        }
    }

    function Get-DefaultInstallDir([string]$Platform) {
        if ($Platform -like "win32-*") {
            return (Join-Path $env:LOCALAPPDATA "Programs\Oxide")
        }
        return (Join-Path $HOME ".local/bin")
    }

    function Test-PathWarning([string]$Dir, [string]$Platform) {
        $sep = if ($Platform -like "win32-*") { ";" } else { ":" }
        $entries = $env:PATH -split [regex]::Escape($sep)
        if ($entries -contains $Dir) { return }
        Write-Warn "$Dir is not on your PATH"
        if ($Platform -like "win32-*") {
            Write-Host "oxide-install: add it with: setx PATH `"$Dir;%PATH%`""
        } else {
            Write-Host "oxide-install: add it with: export PATH=`"$Dir`:`$PATH`""
        }
    }

    $platform = Get-Platform
    $url = Resolve-Url $platform
    $archiveName = ($url -split "/")[-1]

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("oxide-install-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tmp -Force | Out-Null

    try {
        $archivePath = Join-Path $tmp $archiveName
        Write-Info "downloading $archiveName for $platform"
        Get-Binary $url $archivePath

        $sumsPath = Join-Path $tmp "$archiveName.sha256"
        $haveSums = $true
        try {
            Get-Binary "$url.sha256" $sumsPath
        } catch {
            $haveSums = $false
            Write-Warn "checksum file unavailable; skipping verification"
        }
        if ($haveSums) {
            Test-Checksum $archivePath $sumsPath
        }

        if ($platform -like "win32-*") {
            Expand-Archive -Path $archivePath -DestinationPath $tmp -Force
            $binary = "oxide.exe"
        } else {
            & tar -xzf $archivePath -C $tmp
            if ($LASTEXITCODE -ne 0) { throw "failed to extract $archiveName" }
            $binary = "oxide"
        }

        $installDir = if ($env:OXIDE_INSTALL_DIR) { $env:OXIDE_INSTALL_DIR } else { Get-DefaultInstallDir $platform }
        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        $dest = Join-Path $installDir $binary
        Move-Item -Force (Join-Path $tmp $binary) $dest
        if ($platform -notlike "win32-*") {
            & chmod 0755 $dest
        }

        Write-Info "installed Oxide to $dest"
        Test-PathWarning $installDir $platform
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}
