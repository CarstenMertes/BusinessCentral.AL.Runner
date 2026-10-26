// The .NET tool MSDyn365BC.AL.Runner (command al-runner) for AL Runner v3: runs the native al-runner
// binary that the package carries for this OS and CPU, with the same arguments, standard streams and
// exit code. Tooling only: nothing AL code can see (ADR-009).
//
// On Linux and macOS the wrapper replaces itself with the binary (execve), so the binary keeps the
// wrapper's process id: signals, exit codes and the terminal reach it exactly as if it had been started
// directly. On Windows, and when execve fails, the binary runs as a child process: the wrapper waits for
// it and exits with its exit code. Ctrl+C reaches the child from the console (or terminal) itself, so
// the wrapper only ignores it; on Unix it forwards SIGTERM and SIGHUP.

using System.Diagnostics;
using System.Runtime.InteropServices;

namespace AlRunner.Tool;

internal static unsafe class Program
{
    // The binary prints a note when it finds another al-runner on PATH, and it would take this tool's
    // own command in ~/.dotnet/tools for AL Runner v2 (path_note.rs). Off by default under the wrapper;
    // a value the user set is kept.
    private const string NoPathNote = "ALRUNNER_NO_PATH_NOTE";

    // Test hook (packaging/dotnet-tool/test.sh): "1" uses the child-process path on Unix too, so the
    // path Windows takes is tested on Linux.
    private const string ForceSpawn = "ALRUNNER_DOTNET_TOOL_SPAWN";

    // Release targets of build-packages.yml, by .NET runtime identifier.
    private static readonly string[] Shipped = ["linux-x64", "linux-arm64", "osx-arm64", "win-x64"];

    private const int UsageError = 2; // al-runner's own exit code for an error before any test runs

    private static int Main(string[] args)
    {
        var platform = Platform();
        var rid = Pick(platform);
        var exe = rid is null
            ? null
            : Path.Combine(AppContext.BaseDirectory, "native", rid, OperatingSystem.IsWindows() ? "al-runner.exe" : "al-runner");
        if (exe is null || !File.Exists(exe))
        {
            Console.Error.WriteLine(
                $"error: this al-runner package has no binary for {platform}. It has binaries for " +
                $"{string.Join(", ", Shipped)}. Other ways to install: https://aldev.tools/al-runner/getting-started/");
            return UsageError;
        }

        var setNote = string.IsNullOrEmpty(Environment.GetEnvironmentVariable(NoPathNote));
        if (!OperatingSystem.IsWindows())
        {
            if (!EnsureExecutable(exe))
            {
                return UsageError;
            }
            if (Environment.GetEnvironmentVariable(ForceSpawn) != "1")
            {
                // Returns only when execve failed; the child-process path is the fallback.
                Exec(exe, args, setNote);
            }
        }
        return Spawn(exe, args, setNote);
    }

    private static string Platform()
    {
        var os = OperatingSystem.IsWindows() ? "win"
            : OperatingSystem.IsMacOS() ? "osx"
            : OperatingSystem.IsLinux() ? "linux"
            : RuntimeInformation.OSDescription;
        // OSArchitecture, not ProcessArchitecture: an x64 .NET under emulation still reports the real CPU.
        var arch = RuntimeInformation.OSArchitecture.ToString().ToLowerInvariant();
        return $"{os}-{arch}";
    }

    private static string? Pick(string platform) => platform switch
    {
        "linux-x64" => "linux-x64", // the musl build is static: it runs on glibc and musl systems
        "linux-arm64" => "linux-arm64", // likewise static
        "osx-arm64" => "osx-arm64",
        "win-x64" => "win-x64",
        "win-arm64" => "win-x64", // Windows 11 on Arm runs x64 programs
        _ => null,
    };

    // NuGet does not keep the Unix execute bit, so the first run sets it.
    private static bool EnsureExecutable(string exe)
    {
        const UnixFileMode exec = UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute;
        try
        {
            var mode = File.GetUnixFileMode(exe);
            if ((mode & UnixFileMode.UserExecute) == 0)
            {
                File.SetUnixFileMode(exe, mode | exec);
            }
            return true;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Console.Error.WriteLine($"error: could not make {exe} executable: {e.Message}");
            return false;
        }
    }

    // execve and kill come from the C library the runtime already loaded; looking them up through the
    // main program handle works with glibc, musl and macOS alike.
    private static nint Libc(string name) => NativeLibrary.GetExport(NativeLibrary.GetMainProgramHandle(), name);

    private static void Exec(string exe, string[] args, bool setNote)
    {
        var env = new List<string>();
        foreach (System.Collections.DictionaryEntry e in Environment.GetEnvironmentVariables())
        {
            env.Add($"{e.Key}={e.Value}");
        }
        if (setNote)
        {
            env.RemoveAll(e => e.StartsWith(NoPathNote + "=", StringComparison.Ordinal));
            env.Add($"{NoPathNote}=1");
        }
        var argv = new List<string> { exe };
        argv.AddRange(args);

        var path = Marshal.StringToCoTaskMemUTF8(exe);
        var argvp = Strings(argv);
        var envp = Strings(env);
        var execve = (delegate* unmanaged<nint, nint*, nint*, int>)Libc("execve");
        execve(path, argvp, envp);
        var errno = Marshal.GetLastSystemError();
        Console.Error.WriteLine($"note: could not replace the .NET tool process with {exe} (errno {errno}); running it as a child process");
        // The memory is freed when the process exits; nothing else uses it.
    }

    private static nint* Strings(List<string> items)
    {
        var p = (nint*)NativeMemory.Alloc((nuint)(items.Count + 1), (nuint)sizeof(nint));
        for (var i = 0; i < items.Count; i++)
        {
            p[i] = Marshal.StringToCoTaskMemUTF8(items[i]);
        }
        p[items.Count] = 0;
        return p;
    }

    private static int Spawn(string exe, string[] args, bool setNote)
    {
        var psi = new ProcessStartInfo(exe) { UseShellExecute = false };
        foreach (var a in args)
        {
            psi.ArgumentList.Add(a);
        }
        if (setNote)
        {
            psi.Environment[NoPathNote] = "1";
        }
        using var child = Process.Start(psi);
        if (child is null)
        {
            Console.Error.WriteLine($"error: could not start {exe}");
            return UsageError;
        }

        var registrations = new List<PosixSignalRegistration>
        {
            // The console (Windows) or the terminal's process group (Unix) delivers Ctrl+C and
            // Ctrl+Break or Ctrl+\ to the child too: the wrapper waits for the child to decide.
            PosixSignalRegistration.Create(PosixSignal.SIGINT, c => c.Cancel = true),
            PosixSignalRegistration.Create(PosixSignal.SIGQUIT, c => c.Cancel = true),
        };
        if (!OperatingSystem.IsWindows())
        {
            var kill = (delegate* unmanaged<int, int, int>)Libc("kill");
            var pid = child.Id;
            foreach (var (signal, number) in new[] { (PosixSignal.SIGTERM, 15), (PosixSignal.SIGHUP, 1) })
            {
                registrations.Add(PosixSignalRegistration.Create(signal, c =>
                {
                    c.Cancel = true;
                    kill(pid, number);
                }));
            }
        }

        child.WaitForExit();
        foreach (var r in registrations)
        {
            r.Dispose();
        }
        // On Unix a child ended by a signal reports 128 + the signal number, as a shell does.
        return child.ExitCode;
    }
}
