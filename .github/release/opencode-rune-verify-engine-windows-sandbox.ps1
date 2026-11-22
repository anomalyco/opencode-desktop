# Verifies that a signed Windows browser engine bundle boots under CEF's bootstrap.exe
# (CheckDllCodeSigning passes) and runs renderer subprocesses with Untrusted (0x0000) integrity
# and restricted SIDs.
param(
  [Parameter(Mandatory)][string]$EngineDir
)
$ErrorActionPreference = 'Stop'

$exe = Join-Path $EngineDir 'opencode-browser-engine.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw "Missing engine executable at $exe." }

& icacls $EngineDir /grant '*S-1-15-2-2:(OI)(CI)(RX)' /grant '*S-1-15-2-1:(OI)(CI)(RX)' /T /Q
if ($LASTEXITCODE -ne 0) { throw "icacls LPAC RX grant failed on $EngineDir." }

Add-Type -TypeDefinition @'
using System;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class WinSboxCheck {
  [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint a, bool b, int p);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
  [DllImport("advapi32.dll")] static extern bool OpenProcessToken(IntPtr p, uint a, out IntPtr t);
  [DllImport("advapi32.dll")] static extern bool GetTokenInformation(IntPtr t, int c, IntPtr b, int l, out int n);
  [DllImport("advapi32.dll")] static extern IntPtr GetSidSubAuthorityCount(IntPtr s);
  [DllImport("advapi32.dll")] static extern IntPtr GetSidSubAuthority(IntPtr s, uint i);

  public static TcpListener StartHttp(out int port) {
    var tcp = new TcpListener(IPAddress.Loopback, 0);
    tcp.Start();
    port = ((IPEndPoint)tcp.LocalEndpoint).Port;
    var t = new Thread(() => {
      byte[] body = Encoding.UTF8.GetBytes("<!doctype html><title>WinSandboxOK</title><h1>OK</h1>");
      byte[] hdr = Encoding.ASCII.GetBytes(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: " + body.Length + "\r\nConnection: close\r\n\r\n");
      try {
        while (true) {
          var cli = tcp.AcceptTcpClient();
          ThreadPool.QueueUserWorkItem(_ => {
            var buf = new byte[4096];
            try {
              cli.ReceiveTimeout = 2000;
              using (cli)
              using (var st = cli.GetStream()) {
                int n = st.Read(buf, 0, buf.Length);
                if (n > 0) {
                  st.Write(hdr, 0, hdr.Length);
                  st.Write(body, 0, body.Length);
                }
              }
            } catch {}
          });
        }
      } catch {}
    });
    t.IsBackground = true;
    t.Start();
    return tcp;
  }

  public static string Inspect(int pid) {
    IntPtr hp = OpenProcess(0x1000, false, pid);
    if (hp == IntPtr.Zero) return "err_proc";
    try {
      IntPtr ht;
      if (!OpenProcessToken(hp, 8, out ht)) return "err_tok";
      try {
        int n = 0;
        GetTokenInformation(ht, 25, IntPtr.Zero, 0, out n);
        IntPtr b = Marshal.AllocHGlobal(n);
        uint rid = 0xFFFF;
        try {
          if (GetTokenInformation(ht, 25, b, n, out n)) {
            IntPtr sid = Marshal.ReadIntPtr(b, 0);
            byte cnt = Marshal.ReadByte(GetSidSubAuthorityCount(sid));
            rid = (uint)Marshal.ReadInt32(GetSidSubAuthority(sid, (uint)(cnt - 1)));
          }
        } finally { Marshal.FreeHGlobal(b); }
        int rn = 0;
        GetTokenInformation(ht, 11, IntPtr.Zero, 0, out rn);
        int rs = 0;
        if (rn > 0) {
          IntPtr rb = Marshal.AllocHGlobal(rn);
          try { if (GetTokenInformation(ht, 11, rb, rn, out rn)) rs = Marshal.ReadInt32(rb, 0); }
          finally { Marshal.FreeHGlobal(rb); }
        }
        return rid + ":" + rs;
      } finally { CloseHandle(ht); }
    } finally { CloseHandle(hp); }
  }
}
'@

$probe = Join-Path ([IO.Path]::GetTempPath()) "win-sandbox-probe-$PID"
$prof = Join-Path $probe 'profile'
$sockPath = Join-Path $probe 'e.sock'
New-Item -ItemType Directory -Force -Path $prof | Out-Null

$port = 0
$tcp = [WinSboxCheck]::StartHttp([ref]$port)

$psi = [Diagnostics.ProcessStartInfo]::new($exe, "--socket `"$sockPath`" --lang en-US")
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.Environment['OPENCODE_BROWSER_PROFILE'] = $prof
$psi.Environment['OPENCODE_RUNE_OTEL'] = '0'
$proc = [Diagnostics.Process]::Start($psi)
try {
  $first = $proc.StandardOutput.ReadLine()
  if ($first -ne 'listening') {
    throw "Expected 'listening', got '$first': $($proc.StandardError.ReadToEnd())"
  }
  $sock = [Net.Sockets.Socket]::new(
    [Net.Sockets.AddressFamily]::Unix,
    [Net.Sockets.SocketType]::Stream,
    [Net.Sockets.ProtocolType]::Unspecified
  )
  $sock.ReceiveTimeout = 25000
  $sock.Connect([Net.Sockets.UnixDomainSocketEndPoint]::new($sockPath))
  $ns = [Net.Sockets.NetworkStream]::new($sock, $true)
  $wr = [IO.StreamWriter]::new($ns, [Text.UTF8Encoding]::new($false))
  $wr.NewLine = "`n"
  $wr.AutoFlush = $true
  $rd = [IO.StreamReader]::new($ns, [Text.UTF8Encoding]::new($false))

  $hello = @{ type = 'hello'; mach_service = $null; pid = $PID; prompts = $true; extensions = $false; download_save = $true }
  $wr.WriteLine(($hello | ConvertTo-Json -Compress))
  $titleOk = $false
  $browserPid = $proc.Id
  $dl = [DateTime]::UtcNow.AddSeconds(25)
  while ([DateTime]::UtcNow -lt $dl -and -not $titleOk) {
    $line = $rd.ReadLine()
    if (-not $line) { break }
    $m = $line | ConvertFrom-Json
    if ($m.type -eq 'ready') {
      if ($m.pid) { $browserPid = [int]$m.pid }
      $open = @{ type = 'open_tab'; tab_id = 1; url = "http://127.0.0.1:$port/"; width_pt = 640.0; height_pt = 480.0; scale = 1.0; focused = $true }
      $wr.WriteLine(($open | ConvertTo-Json -Compress))
    } elseif ($m.type -eq 'tab_update' -and $m.tab.title -eq 'WinSandboxOK') {
      $titleOk = $true
    }
  }
  if (-not $titleOk) { throw 'Signed Windows x64 sandboxed page did not load.' }
  Start-Sleep -Milliseconds 500
  $renderers = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $browserPid" | Where-Object { $_.CommandLine -match '--type=renderer' })
  if ($renderers.Count -eq 0) { throw 'No sandboxed renderer process found.' }
  foreach ($r in $renderers) {
    $res = [WinSboxCheck]::Inspect([int]$r.ProcessId)
    Write-Host "Windows sandbox verified: renderer=$($r.ProcessId) integrity_rid:restricted_sids=$res"
    $parts = $res -split ':'
    if ([int]$parts[0] -ne 0 -or [int]$parts[1] -lt 1) {
      throw "Renderer $($r.ProcessId) not sandboxed ($res)."
    }
  }
  $wr.WriteLine((@{ type = 'shutdown' } | ConvertTo-Json -Compress))
  $ns.Close()
} finally {
  $tcp.Stop()
  if (-not $proc.WaitForExit(10000)) { $proc.Kill($true) }
  Remove-Item -Recurse -Force $probe -ErrorAction SilentlyContinue
}
