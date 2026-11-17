#include <errno.h>
#include <stdint.h>
#include <sys/sysctl.h>

/* Public SDK fields keep the native kinfo_proc layout out of Rust. */
int zeroshot_macos_process_identity(pid_t pid, uint32_t *parent,
                                   uint32_t *group, uint64_t *born) {
    int mib[] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, pid};
    struct kinfo_proc info;
    size_t length = sizeof(info);
    if (sysctl(mib, 4, &info, &length, NULL, 0) == -1) {
        return errno;
    }
    if (length == 0) {
        return ESRCH;
    }
    if (length != sizeof(info) || info.kp_proc.p_pid != pid ||
        info.kp_eproc.e_ppid < 0 || info.kp_eproc.e_pgid < 0 ||
        info.kp_proc.p_starttime.tv_sec < 0 ||
        info.kp_proc.p_starttime.tv_usec < 0 ||
        info.kp_proc.p_starttime.tv_usec >= 1000000) {
        return -1;
    }
    uint64_t seconds = (uint64_t)info.kp_proc.p_starttime.tv_sec;
    uint64_t micros = (uint64_t)info.kp_proc.p_starttime.tv_usec;
    if (seconds > (UINT64_MAX - micros) / 1000000) {
        return -1;
    }
    uint64_t stamp = seconds * 1000000 + micros;
    if (stamp == 0) {
        return -1;
    }
    *parent = (uint32_t)info.kp_eproc.e_ppid;
    *group = (uint32_t)info.kp_eproc.e_pgid;
    *born = stamp;
    return 0;
}
