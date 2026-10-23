using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace Zenith.DevTools
{
    // The controller owns only processes explicitly assigned to this job.
    // Closing its handle also cleans up Cargo, Vite and the app, not other Relay instances.
    public sealed class ManagedJob : IDisposable
    {
        private SafeFileHandle handle;

        public ManagedJob()
        {
            handle = new SafeFileHandle(CreateJobObject(IntPtr.Zero, null), true);
            if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
            var limits = new ExtendedLimits();
            limits.Basic.LimitFlags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            int size = Marshal.SizeOf(typeof(ExtendedLimits));
            IntPtr data = Marshal.AllocHGlobal(size);
            try
            {
                Marshal.StructureToPtr(limits, data, false);
                if (!SetInformationJobObject(handle, 9, data, (uint)size))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
            }
            catch { handle.Dispose(); throw; }
            finally { Marshal.FreeHGlobal(data); }
        }

        public void Add(Process process)
        {
            if (!AssignProcessToJobObject(handle, process.Handle))
                throw new Win32Exception(Marshal.GetLastWin32Error());
        }

        public bool Contains(Process process)
        {
            try
            {
                bool inside;
                return IsProcessInJob(process.Handle, handle, out inside) && inside;
            }
            catch (InvalidOperationException) { return false; }
            catch (Win32Exception) { return false; }
        }

        public uint ActiveProcesses
        {
            get
            {
                BasicAccounting accounting;
                uint returned;
                if (!QueryInformationJobObject(handle, 1, out accounting,
                    (uint)Marshal.SizeOf(typeof(BasicAccounting)), out returned))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                return accounting.ActiveProcesses;
            }
        }

        public void Stop()
        {
            if (!TerminateJobObject(handle, 1))
                throw new Win32Exception(Marshal.GetLastWin32Error());
        }

        public void Dispose() { if (handle != null) handle.Dispose(); }

        [StructLayout(LayoutKind.Sequential)]
        private struct BasicLimits
        {
            public long PerProcessUserTimeLimit, PerJobUserTimeLimit;
            public uint LimitFlags;
            public UIntPtr MinimumWorkingSetSize, MaximumWorkingSetSize;
            public uint ActiveProcessLimit;
            public UIntPtr Affinity;
            public uint PriorityClass, SchedulingClass;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct IoCounters
        {
            public ulong ReadOperationCount, WriteOperationCount, OtherOperationCount;
            public ulong ReadTransferCount, WriteTransferCount, OtherTransferCount;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct ExtendedLimits
        {
            public BasicLimits Basic;
            public IoCounters Io;
            public UIntPtr ProcessMemoryLimit, JobMemoryLimit;
            public UIntPtr PeakProcessMemoryUsed, PeakJobMemoryUsed;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct BasicAccounting
        {
            public long TotalUserTime, TotalKernelTime, ThisPeriodTotalUserTime, ThisPeriodTotalKernelTime;
            public uint TotalPageFaultCount, TotalProcesses, ActiveProcesses, TotalTerminatedProcesses;
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr CreateJobObject(IntPtr attributes, string name);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool SetInformationJobObject(SafeFileHandle job, int kind, IntPtr data, uint size);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool AssignProcessToJobObject(SafeFileHandle job, IntPtr process);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool IsProcessInJob(IntPtr process, SafeFileHandle job, out bool inside);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool QueryInformationJobObject(SafeFileHandle job, int kind,
            out BasicAccounting accounting, uint size, out uint returned);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool TerminateJobObject(SafeFileHandle job, uint exitCode);
    }
}
