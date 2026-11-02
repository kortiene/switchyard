// Linux x86_64, test-only bridge policy. Runs in the dynamic loader, before
// Node starts threads or loads JavaScript. No preload failure may reach Pi:
// guard.cjs independently requires the constructor's success marker.
#define _GNU_SOURCE
#include <errno.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/sched.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

#if !defined(__x86_64__) || defined(__ILP32__)
#error "bridge lockdown supports Linux x86_64 only"
#endif

#define DENY(n) BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_##n, 0, 1), \
    BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM)

__attribute__((constructor)) static void lockdown(void) {
    const uint32_t thread = CLONE_VM | CLONE_SIGHAND | CLONE_THREAD;
    const uint32_t namespaces = CLONE_NEWCGROUP | CLONE_NEWIPC | CLONE_NEWNET |
        CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWTIME | CLONE_NEWUSER | CLONE_NEWUTS;
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JSET | BPF_K, 0x40000000, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        // glibc falls back to clone for pthreads; BPF cannot inspect clone3's
        // pointed-to arguments, so never allow clone3 itself.
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone3, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | ENOSYS),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone, 0, 7),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, args[0])),
        BPF_JUMP(BPF_JMP | BPF_JSET | BPF_K, namespaces, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_ALU | BPF_AND | BPF_K, thread),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, thread, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
        // Reload is the target for non-clone syscalls.
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        DENY(fork), DENY(vfork), DENY(execve), DENY(execveat),
        DENY(socket), DENY(socketpair), DENY(connect), DENY(bind),
        DENY(listen), DENY(accept), DENY(accept4), DENY(sendto), DENY(sendmsg),
        DENY(sendmmsg), DENY(recvfrom), DENY(recvmsg), DENY(recvmmsg),
        DENY(io_uring_setup), DENY(io_uring_enter), DENY(io_uring_register),
        DENY(unshare), DENY(setns), DENY(mount), DENY(umount2),
        DENY(pivot_root), DENY(chroot), DENY(open_by_handle_at),
        DENY(ptrace), DENY(process_vm_readv), DENY(process_vm_writev),
        DENY(pidfd_getfd), DENY(bpf), DENY(perf_event_open),
        DENY(userfaultfd), DENY(keyctl), DENY(add_key), DENY(request_key),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = { .len = sizeof(filter) / sizeof(filter[0]), .filter = filter };
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 ||
        prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program) != 0 ||
        setenv("SWITCHYARD_BRIDGE_LOCKDOWN", "v1", 1) != 0) {
        static const char message[] = "bridge lockdown installation failed\n";
        ssize_t written = write(STDERR_FILENO, message, sizeof(message) - 1);
        (void)written;
        _exit(125);
    }
}

#ifdef LOCKDOWN_TEST
#include <assert.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <sys/socket.h>
static void *thread_test(void *value) {
    assert(syscall(SYS_fork) == -1 && errno == EPERM);
    assert(syscall(SYS_socket, AF_INET, SOCK_STREAM, 0) == -1 && errno == EPERM);
    return value;
}
int main(void) {
    assert(syscall(SYS_fork) == -1 && errno == EPERM);
    assert(syscall(SYS_vfork) == -1 && errno == EPERM);
    assert(syscall(SYS_clone, SIGCHLD, 0, 0, 0, 0) == -1 && errno == EPERM);
    assert(syscall(SYS_clone3, 0, 0) == -1 && errno == ENOSYS);
    assert(syscall(SYS_execve, "/bin/true", 0, 0) == -1 && errno == EPERM);
    assert(syscall(SYS_execveat, -1, "", 0, 0, 0) == -1 && errno == EPERM);
    assert(syscall(SYS_socket, AF_UNIX, SOCK_STREAM, 0) == -1 && errno == EPERM);
    assert(syscall(SYS_io_uring_setup, 0, 0) == -1 && errno == EPERM);
    assert(syscall(SYS_unshare, CLONE_NEWUSER) == -1 && errno == EPERM);
    pthread_t thread_id;
    assert(pthread_create(&thread_id, NULL, thread_test, NULL) == 0);
    assert(pthread_join(thread_id, NULL) == 0);
    puts("lockdown: raw syscalls denied; pthread works and inherits filter");
}
#endif
