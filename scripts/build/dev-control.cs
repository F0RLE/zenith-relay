using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Forms;

namespace Zenith.DevTools
{
    internal static class DevControl
    {
        private const string SyntheticOutput = "synthetic-\u0421\u0442\u0440\u043e\u043a\u0430-\ud83d\ude00";

        [STAThread]
        private static int Main(string[] args)
        {
            if (args.Contains("--test-echo"))
            {
                byte[] bytes = Encoding.UTF8.GetBytes(SyntheticOutput);
                Console.OpenStandardOutput().Write(bytes, 0, bytes.Length);
                Console.OpenStandardError().Write(bytes, 0, bytes.Length);
                return 7;
            }
            if (args.Contains("--test-leaf")) { Thread.Sleep(60000); return 0; }
            if (args.Contains("--test-child"))
            {
                if (Console.ReadLine() != "start") return 1;
                Process.Start(new ProcessStartInfo(Assembly.GetExecutingAssembly().Location, "--test-leaf") {
                    UseShellExecute = false, CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden
                });
                Thread.Sleep(60000);
                return 0;
            }
            if (args.Contains("--run-owned")) return RunOwned(args);
            bool check = args.Contains("--check");
            string report = ValueAfter(args, "--report");
            try
            {
                string repo = ValueAfter(args, "--repo") ?? Path.Combine(
                    AppDomain.CurrentDomain.BaseDirectory, "zenith-platform-workspace", "zenith-relay");
                Application.EnableVisualStyles();
                Application.SetCompatibleTextRenderingDefault(false);
                using (var form = new DevControlForm(Path.GetFullPath(repo)))
                {
                    if (check)
                    {
                        if (args.Contains("--self-test")) VerifyManagedJobs();
                        form.ValidateIdle();
                        string image = ValueAfter(args, "--render");
                        if (image != null)
                        {
                            // Native RichTextBox/buttons need real handles before WM_PRINT.
                            form.ShowInTaskbar = false;
                            form.Opacity = 0;
                            form.Show();
                            Application.DoEvents();
                            form.PerformLayout();
                            using (var bitmap = new Bitmap(form.Width, form.Height))
                            {
                                form.DrawToBitmap(bitmap, new Rectangle(Point.Empty, form.Size));
                                bitmap.Save(image);
                            }
                            form.Hide();
                        }
                        form.ValidateIdle();
                        WriteReport(report, new { ok = true, buttons = form.ButtonTexts,
                            realRelayStarts = form.SessionStartCount, selfTest = args.Contains("--self-test") });
                        return 0;
                    }
                    bool created;
                    using (var mutex = new Mutex(true, "Local\\ZenithRelayDevControl", out created))
                    {
                        if (!created) { MessageBox.Show(form.Message("alreadyOpen"), form.Text); return 0; }
                        try { Application.Run(form); }
                        finally { mutex.ReleaseMutex(); }
                    }
                }
                return 0;
            }
            catch (Exception error)
            {
                if (check) WriteReport(report, new { ok = false, error = error.ToString() });
                else MessageBox.Show(error.Message, "Relay Dev", MessageBoxButtons.OK, MessageBoxIcon.Error);
                return 1;
            }
        }

        private static string ValueAfter(string[] args, string flag)
        {
            int index = Array.IndexOf(args, flag);
            if (index < 0) return null;
            if (index + 1 >= args.Length) throw new ArgumentException("Missing value for " + flag);
            return args[index + 1];
        }

        private static void WriteReport(string path, object value)
        {
            if (path != null) File.WriteAllText(path, new JavaScriptSerializer().Serialize(value), Encoding.UTF8);
        }

        private static Process Fixture()
        {
            return Process.Start(new ProcessStartInfo(Assembly.GetExecutingAssembly().Location, "--test-child") {
                UseShellExecute = false, CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden,
                RedirectStandardInput = true
            });
        }

        internal static void ResumeOwned(Process process)
        {
            // The bootstrap cannot launch Cargo/Vite before its job assignment succeeds.
            process.StandardInput.WriteLine("start");
            process.StandardInput.Close();
        }

        internal static string Quote(string value) { return "\"" + value.Replace("\"", "\\\"") + "\""; }

        private static int RunOwned(string[] args)
        {
            Console.SetOut(new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false)) { AutoFlush = true });
            Console.SetError(new StreamWriter(Console.OpenStandardError(), new UTF8Encoding(false)) { AutoFlush = true });
            if (Console.ReadLine() != "start") return 1;
            try
            {
                using (var child = new Process { StartInfo = new ProcessStartInfo(
                    ValueAfter(args, "--bun"), args.Contains("--test-owned") ? "--test-echo" : "scripts/build/tauri-dev.mjs --no-watch") {
                    WorkingDirectory = ValueAfter(args, "--repo"), UseShellExecute = false,
                    CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden,
                    RedirectStandardOutput = true, RedirectStandardError = true,
                    StandardOutputEncoding = Encoding.UTF8, StandardErrorEncoding = Encoding.UTF8
                }})
                {
                    child.OutputDataReceived += delegate(object sender, DataReceivedEventArgs item) {
                        if (item.Data != null) Console.Out.WriteLine(item.Data);
                    };
                    child.ErrorDataReceived += delegate(object sender, DataReceivedEventArgs item) {
                        if (item.Data != null) Console.Error.WriteLine(item.Data);
                    };
                    child.Start();
                    child.BeginOutputReadLine(); child.BeginErrorReadLine();
                    child.WaitForExit();
                    return child.ExitCode;
                }
            }
            catch (Exception error) { Console.Error.WriteLine(error.Message); return 1; }
        }

        private static void WaitForTree(ManagedJob job)
        {
            for (int i = 0; i < 100 && job.ActiveProcesses < 2; i++) Thread.Sleep(50);
            if (job.ActiveProcesses < 2) throw new Exception("Fixture descendant was not assigned to its parent's job.");
        }

        private static void VerifyManagedJobs()
        {
            // Only synthetic copies of this executable are started/stopped in these tests.
            using (var job = new ManagedJob())
            using (var child = Fixture())
            {
                job.Add(child);
                if (!job.Contains(child) || job.Contains(Process.GetCurrentProcess()))
                    throw new Exception("Managed-job ownership check failed.");
                ResumeOwned(child);
                WaitForTree(job);
                job.Stop();
                if (!child.WaitForExit(5000)) throw new Exception("Owned fixture did not stop.");
                for (int i = 0; i < 100 && job.ActiveProcesses != 0; i++) Thread.Sleep(50);
                if (job.ActiveProcesses != 0) throw new Exception("Owned descendants survived stop.");
            }
            var closingJob = new ManagedJob();
            using (var child = Fixture())
            {
                try { closingJob.Add(child); ResumeOwned(child); WaitForTree(closingJob); }
                finally { closingJob.Dispose(); }
                if (!child.WaitForExit(5000)) throw new Exception("Closing the job did not stop its fixture.");
            }
            string fixtureArgs = "--run-owned --test-owned --repo " + Quote(Environment.CurrentDirectory)
                + " --bun " + Quote(Assembly.GetExecutingAssembly().Location);
            using (var commandJob = new ManagedJob())
            using (var command = new Process { StartInfo = new ProcessStartInfo(
                Assembly.GetExecutingAssembly().Location, fixtureArgs) {
                UseShellExecute = false, CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden,
                RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true,
                StandardOutputEncoding = Encoding.UTF8, StandardErrorEncoding = Encoding.UTF8
            }})
            {
                command.Start();
                commandJob.Add(command);
                ResumeOwned(command);
                string stdout = command.StandardOutput.ReadToEnd();
                string stderr = command.StandardError.ReadToEnd();
                if (!command.WaitForExit(5000) || command.ExitCode != 7
                    || stdout.TrimEnd('\r', '\n') != SyntheticOutput || stderr.TrimEnd('\r', '\n') != SyntheticOutput)
                    throw new Exception("Owned command changed UTF-8 output or its exit code.");
            }
        }
    }

    internal sealed class DevControlForm : Form
    {
        private readonly string repo;
        private readonly string logDirectory;
        private readonly Dictionary<string, string> messages;
        private readonly ConcurrentQueue<string> pendingLog = new ConcurrentQueue<string>();
        private readonly Button startButton = new Button(), rebuildButton = new Button();
        private readonly Button stopButton = new Button(), logsButton = new Button();
        private readonly Label status = new Label();
        private readonly TextBox output = new TextBox();
        private readonly System.Windows.Forms.Timer timer = new System.Windows.Forms.Timer();
        private Process process;
        private ManagedJob job;
        private string state = "Idle", logPath;
        private bool restartPending, closePending, allowClose;

        public DevControlForm(string root)
        {
            repo = root;
            messages = new JavaScriptSerializer().Deserialize<Dictionary<string, string>>(File.ReadAllText(
                Path.Combine(repo, "scripts", "build", "dev-control.messages.json"), Encoding.UTF8));
            logDirectory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Zenith-Relay-Dev-Control");
            BuildControls();
            startButton.Click += delegate { StartSession(); };
            rebuildButton.Click += delegate {
                if (process == null) StartSession();
                else if (Confirm(Message("confirm"))) StopSession(true);
            };
            stopButton.Click += delegate { if (Confirm(Message("confirm"))) StopSession(false); };
            logsButton.Click += delegate {
                if (Directory.Exists(logDirectory)) Process.Start(new ProcessStartInfo("explorer.exe", Quote(logDirectory)) {
                    WindowStyle = ProcessWindowStyle.Normal
                });
            };
            timer.Interval = 750;
            timer.Tick += delegate { Poll(); };
            timer.Start();
        }

        public string Message(string key) { return messages[key].Replace("\\r\\n", "\r\n"); }
        public int SessionStartCount { get; private set; }
        public string[] ButtonTexts { get { return new[] { startButton.Text, rebuildButton.Text, stopButton.Text, logsButton.Text }; } }
        private static string Quote(string value) { return DevControl.Quote(value); }

        private void BuildControls()
        {
            Text = Message("title"); Size = new Size(1040, 720); MinimumSize = new Size(900, 580);
            StartPosition = FormStartPosition.CenterScreen; AutoScaleMode = AutoScaleMode.Dpi;
            BackColor = ColorTranslator.FromHtml("#111827"); ForeColor = ColorTranslator.FromHtml("#E5E7EB");
            Font = new Font("Segoe UI", 10);
            var layout = new TableLayoutPanel { Dock = DockStyle.Fill, Padding = new Padding(20), ColumnCount = 1, RowCount = 6 };
            foreach (int height in new[] { 42, 38, 56, 36 }) layout.RowStyles.Add(new RowStyle(SizeType.Absolute, height));
            layout.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            layout.RowStyles.Add(new RowStyle(SizeType.Absolute, 30));
            Controls.Add(layout);
            layout.Controls.Add(new Label { Text = Message("heading"), Dock = DockStyle.Fill,
                Font = new Font("Segoe UI", 18, FontStyle.Bold) }, 0, 0);
            layout.Controls.Add(new Label { Text = Message("subtitle"), Dock = DockStyle.Fill }, 0, 1);
            var buttons = new FlowLayoutPanel { Dock = DockStyle.Fill, WrapContents = false };
            layout.Controls.Add(buttons, 0, 2);
            Button[] controls = { startButton, rebuildButton, stopButton, logsButton };
            string[] keys = { "start", "rebuild", "stop", "logs" };
            int[] widths = { 140, 270, 145, 150 };
            for (int i = 0; i < controls.Length; i++)
            {
                Button button = controls[i]; button.Text = Message(keys[i]); button.Size = new Size(widths[i], 42);
                button.Margin = new Padding(0, 0, 10, 0); button.FlatStyle = FlatStyle.Flat;
                button.BackColor = ColorTranslator.FromHtml(i == 1 ? "#2563EB" : "#243247");
                button.ForeColor = Color.White; buttons.Controls.Add(button);
            }
            status.Dock = DockStyle.Fill; status.AutoEllipsis = true;
            status.ForeColor = ColorTranslator.FromHtml("#86EFAC"); layout.Controls.Add(status, 0, 3);
            output.Dock = DockStyle.Fill; output.ReadOnly = true; output.WordWrap = false;
            output.Multiline = true; output.ScrollBars = ScrollBars.Both; output.BorderStyle = BorderStyle.FixedSingle;
            output.BackColor = ColorTranslator.FromHtml("#0B1220"); output.ForeColor = ColorTranslator.FromHtml("#CBD5E1");
            output.Font = new Font("Consolas", 10); output.Text = Message("initialLog"); layout.Controls.Add(output, 0, 4);
            layout.Controls.Add(new Label { Text = Message("footer"), Dock = DockStyle.Fill,
                Font = new Font("Segoe UI", 9), ForeColor = ColorTranslator.FromHtml("#9CA3AF") }, 0, 5);
            SetState("Idle", Message("idle"));
        }

        public void ValidateIdle()
        {
            if (process != null || job != null || state != "Idle" || !startButton.Enabled || stopButton.Enabled)
                throw new Exception("The controller must open idle without starting Relay.");
            if (ButtonTexts.Any(String.IsNullOrWhiteSpace)) throw new Exception("A controller button has no label.");
        }

        private void SetState(string value, string text)
        {
            state = value; status.Text = text;
            startButton.Enabled = value == "Idle"; rebuildButton.Enabled = value != "Stopping";
            stopButton.Enabled = value == "Starting" || value == "Running";
            stopButton.BackColor = ColorTranslator.FromHtml(stopButton.Enabled ? "#243247" : "#9CA3AF");
        }

        private bool Confirm(string text)
        {
            return MessageBox.Show(this, text, Text, MessageBoxButtons.YesNo, MessageBoxIcon.Warning,
                MessageBoxDefaultButton.Button2) == DialogResult.Yes;
        }

        private void Notice(string text) { MessageBox.Show(this, text, Text, MessageBoxButtons.OK, MessageBoxIcon.Information); }

        private static string FindBun()
        {
            var paths = new List<string> { Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                "Development", "bun", "bin", "bun.exe") };
            foreach (string folder in (Environment.GetEnvironmentVariable("PATH") ?? "").Split(';'))
                if (!String.IsNullOrWhiteSpace(folder)) paths.Add(Path.Combine(folder.Trim('"'), "bun.exe"));
            return paths.FirstOrDefault(File.Exists);
        }

        private void StartSession()
        {
            if (process != null) return;
            restartPending = false;
            if (Process.GetProcessesByName("zenith-relay").Length != 0) { Notice(Message("external")); return; }
            var listener = new System.Net.Sockets.TcpListener(System.Net.IPAddress.Loopback, 1420);
            try { listener.Start(); }
            catch (System.Net.Sockets.SocketException) { Notice(Message("portBusy")); return; }
            finally { listener.Stop(); }
            string bun = FindBun();
            if (bun == null) { Notice(Message("missingBun")); return; }
            try
            {
                Directory.CreateDirectory(logDirectory);
                if ((File.GetAttributes(logDirectory) & FileAttributes.ReparsePoint) != 0)
                    throw new IOException("Refusing a linked log directory.");
                logPath = Path.Combine(logDirectory, DateTime.Now.ToString("yyyyMMdd-HHmmss-fff") + ".log");
                job = new ManagedJob();
                Log("\r\n" + Message("startingLog") + "\r\n");
                string arguments = "--run-owned --repo " + Quote(repo) + " --bun " + Quote(bun);
                process = new Process { StartInfo = new ProcessStartInfo(Assembly.GetExecutingAssembly().Location, arguments) {
                    WorkingDirectory = repo, UseShellExecute = false, CreateNoWindow = true,
                    WindowStyle = ProcessWindowStyle.Hidden, RedirectStandardOutput = true, RedirectStandardError = true,
                    RedirectStandardInput = true,
                    StandardOutputEncoding = Encoding.UTF8, StandardErrorEncoding = Encoding.UTF8
                }};
                process.OutputDataReceived += delegate(object sender, DataReceivedEventArgs args) {
                    if (args.Data != null) pendingLog.Enqueue(args.Data + "\r\n");
                };
                process.ErrorDataReceived += delegate(object sender, DataReceivedEventArgs args) {
                    if (args.Data != null) pendingLog.Enqueue(args.Data + "\r\n");
                };
                process.Start();
                SessionStartCount++;
                job.Add(process);
                DevControl.ResumeOwned(process);
                process.BeginOutputReadLine(); process.BeginErrorReadLine();
                Log(String.Format(Message("logLocation"), logPath) + "\r\n");
                SetState("Starting", Message("building"));
            }
            catch (Exception error)
            {
                if (process != null) { try { if (!process.HasExited) process.Kill(); } catch (InvalidOperationException) { } }
                ReleaseSession(); SetState("Idle", Message("failed")); Log(error.Message + "\r\n"); Notice(error.Message);
            }
        }

        private void StopSession(bool restart)
        {
            if (job == null) { if (restart) StartSession(); return; }
            try
            {
                restartPending = restart; job.Stop();
                Log(Message("stoppingLog") + "\r\n"); SetState("Stopping", Message("stopping"));
            }
            catch (Win32Exception error) { restartPending = closePending = false; Notice(error.Message); }
        }

        private void Log(string text)
        {
            text = Regex.Replace(text, "\x1b\\[[0-9;?]*[A-Za-z]", "");
            if (output.TextLength > 200000) output.Text = output.Text.Substring(output.TextLength - 100000);
            output.AppendText(text); output.SelectionStart = output.TextLength; output.ScrollToCaret();
            if (logPath != null)
            {
                try { File.AppendAllText(logPath, text, Encoding.UTF8); }
                catch (IOException) { logPath = null; }
                catch (UnauthorizedAccessException) { logPath = null; }
            }
        }

        private void DrainLog() { string text; while (pendingLog.TryDequeue(out text)) Log(text); }

        private void Poll()
        {
            try
            {
                DrainLog(); if (process == null) return;
                if (process.HasExited)
                {
                    int code = process.ExitCode;
                    if (job.ActiveProcesses != 0) { job.Stop(); return; }
                    process.WaitForExit(); DrainLog(); Log(String.Format(Message("endedLog"), code) + "\r\n");
                    bool restart = restartPending; ReleaseSession(); restartPending = false;
                    SetState("Idle", Message(code != 0 && state != "Stopping" ? "failed" : "finished"));
                    if (closePending) { allowClose = true; Close(); }
                    else if (restart) StartSession();
                }
                else if (state == "Starting")
                {
                    foreach (Process app in Process.GetProcessesByName("zenith-relay"))
                        using (app) if (job.Contains(app)) { SetState("Running", Message("running")); break; }
                }
            }
            catch (Exception error) { Log(error.Message + "\r\n"); }
        }

        private void ReleaseSession()
        {
            if (job != null) { job.Dispose(); job = null; }
            if (process != null) { process.Dispose(); process = null; }
        }

        protected override void OnFormClosing(FormClosingEventArgs args)
        {
            if (!allowClose && process != null)
            {
                args.Cancel = true;
                if (Confirm(Message("confirmClose"))) { closePending = true; StopSession(false); }
            }
            base.OnFormClosing(args);
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing) { timer.Dispose(); ReleaseSession(); }
            base.Dispose(disposing);
        }
    }
}
