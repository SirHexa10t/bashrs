//! A syscall filter (seccomp): the classic-BPF program the kernel runs on every system call a
//! process and its children make ([`program`]), and putting it in place ([`install`]) — for
//! `gm_run_exe`'s sandbox, whose game has no use for the calls it refuses.
//!
//! What it refuses is Flatpak's base filter, the one every Flatpak app runs under, Steam's
//! included: the calls that set up namespaces and mounts, and `clone` asked for a new user
//! namespace; the kernel's keyrings, its log, process accounting, quotas and the NUMA memory calls;
//! faking a terminal's input (`TIOCSTI`, `TIOCLINUX`); and sockets of any family but local, IPv4,
//! IPv6 and netlink. `clone3` and the newer mount calls answer that there's no such call
//! (`ENOSYS`), so that a C library falls back to the `clone` that can be checked. Not the calls
//! Flatpak lifts for development tools and Steam (`ptrace`, `perf_event_open`, `personality`):
//! Wine's server uses `ptrace`.
//!
//! On x86-64 a program can call the kernel three ways, each numbering the calls its own way:
//! x86-64's, i386's (the 32-bit programs Wine runs) and x32's. The filter refuses the same calls
//! both of the first two ways, and every x32 call. A 32-bit program's sockets made through the old
//! `socketcall` can't be told apart by family — their arguments are in memory, out of a filter's
//! sight — and go through, as under Flatpak. Other architectures get no filter yet.

use std::io;

/// One instruction of a classic-BPF program, as the kernel reads it (`struct sock_filter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Insn {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

/// The filter for programs built for `arch` (as [`std::env::consts::ARCH`] spells it), or `None`
/// where there's none for that architecture.
pub(crate) fn program(arch: &str) -> Option<Vec<Insn>> {
    if arch != "x86_64" {
        return None;
    }
    let mut program = vec![load(ARCH)];
    program.extend(when(AUDIT_ARCH_X86_64, checks(&X86_64, true)));
    program.extend(when(AUDIT_ARCH_I386, checks(&I386, false)));
    program.push(ret(KILL_PROCESS)); // no other way to call the kernel exists on x86-64
    Some(program)
}

/// Put `program` in place for this process and every one it starts, past undoing: first
/// no-new-privileges, which an unprivileged process needs before it may filter its calls.
pub(crate) fn install(program: &[Insn]) -> io::Result<()> {
    let mut filters: Vec<libc::sock_filter> =
        program.iter().map(|insn| libc::sock_filter { code: insn.code, jt: insn.jt, jf: insn.jf, k: insn.k }).collect();
    let len = u16::try_from(filters.len()).map_err(|_| io::Error::other("the filter is too long"))?;
    let prog = libc::sock_fprog { len, filter: filters.as_mut_ptr() };
    // SAFETY: two prctl calls; the second reads `prog`, which points at `filters`, both alive
    // until it returns (the kernel copies the program).
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &prog as *const libc::sock_fprog) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The calls one way of calling the kernel numbers its own way, as the filter needs them: those it
/// refuses outright (each name, number and answer), and the three it looks into the arguments of.
struct Calls {
    refused: &'static [(&'static str, u32, u32)],
    clone: u32,
    ioctl: u32,
    socket: u32,
}

/// One architecture's checks, from the call's number on: x32's calls refused (`x32`); then each
/// refused call; `clone` asked for a new user namespace; `ioctl` faking a terminal's input;
/// `socket` of a family not allowed; and everything else let through.
fn checks(calls: &Calls, x32: bool) -> Vec<Insn> {
    let mut block = vec![load(NR)];
    if x32 {
        block.extend([jump(JGE_K, X32_CALL, 0, 1), ret(errno(ENOSYS))]);
    }
    for &(_, number, answer) in calls.refused {
        block.extend(when(number, vec![ret(answer)]));
    }
    block.extend(when(calls.clone, vec![load(ARG0), jump(JSET_K, CLONE_NEWUSER, 0, 1), ret(errno(EPERM)), ret(ALLOW)]));
    let faking = [TIOCSTI, TIOCLINUX].into_iter().flat_map(|request| when(request, vec![ret(errno(EPERM))]));
    block.extend(when(calls.ioctl, std::iter::once(load(ARG1)).chain(faking).chain([ret(ALLOW)]).collect()));
    let families = FAMILIES.iter().flat_map(|&family| when(family, vec![ret(ALLOW)]));
    block.extend(when(calls.socket, std::iter::once(load(ARG0)).chain(families).chain([ret(errno(EAFNOSUPPORT))]).collect()));
    block.push(ret(ALLOW));
    block
}

/// `block`, run where what's loaded is `k`, and jumped over otherwise.
fn when(k: u32, block: Vec<Insn>) -> Vec<Insn> {
    let skip = u8::try_from(block.len()).expect("a block is short enough to jump over");
    std::iter::once(jump(JEQ_K, k, 0, skip)).chain(block).collect()
}

/// Load the 32-bit word at `offset` in the call's data.
const fn load(offset: u32) -> Insn {
    Insn { code: LD_W_ABS, jt: 0, jf: 0, k: offset }
}

/// Jump on what's loaded, compared with `k`: `jt` instructions on where it holds, `jf` where not.
const fn jump(code: u16, k: u32, jt: u8, jf: u8) -> Insn {
    Insn { code, jt, jf, k }
}

/// Answer the call with `k`.
const fn ret(k: u32) -> Insn {
    Insn { code: RET_K, jt: 0, jf: 0, k }
}

/// A filter's answer that fails the call with `code`.
const fn errno(code: u32) -> u32 {
    RET_ERRNO | code
}

// The instructions used: load a word of the call's data; jump if equal, if at least, if any bit
// is set; answer.
const LD_W_ABS: u16 = 0x20;
const JEQ_K: u16 = 0x15;
const JGE_K: u16 = 0x35;
const JSET_K: u16 = 0x45;
const RET_K: u16 = 0x06;

// Where things are in the call's data (`struct seccomp_data`): its number, the architecture it was
// made in, and the lower halves of its first two arguments (little-endian).
const NR: u32 = 0;
const ARCH: u32 = 4;
const ARG0: u32 = 16;
const ARG1: u32 = 24;

// A filter's answers, and the errors it gives.
const ALLOW: u32 = 0x7fff_0000;
const KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const EPERM: u32 = 1;
const ENOSYS: u32 = 38;
const EAFNOSUPPORT: u32 = 97;

// The ways of calling the kernel on x86-64: x86-64's own, and i386's; x32's calls are made as
// x86-64's, their numbers marked by this bit.
const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
const AUDIT_ARCH_I386: u32 = 0x4000_0003;
const X32_CALL: u32 = 0x4000_0000;

/// `clone`'s flag for a new user namespace.
const CLONE_NEWUSER: u32 = 0x1000_0000;
/// The terminal requests that fake its input: pushing a character into it, and the Linux
/// console's selection pasting.
const TIOCSTI: u32 = 0x5412;
const TIOCLINUX: u32 = 0x541C;
/// The socket families allowed: unspecified (refused by the kernel anyway), local, IPv4, IPv6 and
/// netlink.
const FAMILIES: [u32; 5] = [0, 1, 2, 10, 16];

/// x86-64's numbers.
const X86_64: Calls = Calls {
    refused: &[
        ("syslog", 103, errno(EPERM)),
        ("uselib", 134, errno(EPERM)),
        ("acct", 163, errno(EPERM)),
        ("quotactl", 179, errno(EPERM)),
        ("add_key", 248, errno(EPERM)),
        ("keyctl", 250, errno(EPERM)),
        ("request_key", 249, errno(EPERM)),
        ("move_pages", 279, errno(EPERM)),
        ("mbind", 237, errno(EPERM)),
        ("get_mempolicy", 239, errno(EPERM)),
        ("set_mempolicy", 238, errno(EPERM)),
        ("migrate_pages", 256, errno(EPERM)),
        ("unshare", 272, errno(EPERM)),
        ("setns", 308, errno(EPERM)),
        ("mount", 165, errno(EPERM)),
        ("umount2", 166, errno(EPERM)),
        ("pivot_root", 155, errno(EPERM)),
        ("chroot", 161, errno(EPERM)),
        ("clone3", 435, errno(ENOSYS)),
        ("open_tree", 428, errno(ENOSYS)),
        ("move_mount", 429, errno(ENOSYS)),
        ("fsopen", 430, errno(ENOSYS)),
        ("fsconfig", 431, errno(ENOSYS)),
        ("fsmount", 432, errno(ENOSYS)),
        ("fspick", 433, errno(ENOSYS)),
        ("mount_setattr", 442, errno(ENOSYS)),
    ],
    clone: 56,
    ioctl: 16,
    socket: 41,
};

/// i386's numbers: x86-64's calls, and `umount`, which i386 has besides `umount2`.
const I386: Calls = Calls {
    refused: &[
        ("syslog", 103, errno(EPERM)),
        ("uselib", 86, errno(EPERM)),
        ("acct", 51, errno(EPERM)),
        ("quotactl", 131, errno(EPERM)),
        ("add_key", 286, errno(EPERM)),
        ("keyctl", 288, errno(EPERM)),
        ("request_key", 287, errno(EPERM)),
        ("move_pages", 317, errno(EPERM)),
        ("mbind", 274, errno(EPERM)),
        ("get_mempolicy", 275, errno(EPERM)),
        ("set_mempolicy", 276, errno(EPERM)),
        ("migrate_pages", 294, errno(EPERM)),
        ("unshare", 310, errno(EPERM)),
        ("setns", 346, errno(EPERM)),
        ("mount", 21, errno(EPERM)),
        ("umount", 22, errno(EPERM)),
        ("umount2", 52, errno(EPERM)),
        ("pivot_root", 217, errno(EPERM)),
        ("chroot", 61, errno(EPERM)),
        ("clone3", 435, errno(ENOSYS)),
        ("open_tree", 428, errno(ENOSYS)),
        ("move_mount", 429, errno(ENOSYS)),
        ("fsopen", 430, errno(ENOSYS)),
        ("fsconfig", 431, errno(ENOSYS)),
        ("fsmount", 432, errno(ENOSYS)),
        ("fspick", 433, errno(ENOSYS)),
        ("mount_setattr", 442, errno(ENOSYS)),
    ],
    clone: 120,
    ioctl: 54,
    socket: 359,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// What `program` answers a call `nr` made the way `arch` names, its first two arguments
    /// `args` — run here as the kernel runs it.
    fn answer(program: &[Insn], arch: u32, nr: u32, args: [u64; 2]) -> u32 {
        let word = |offset: u32| -> u32 {
            match offset {
                NR => nr,
                ARCH => arch,
                ARG0 => args[0] as u32,
                ARG1 => args[1] as u32,
                other => panic!("the filter reads offset {other}, which this test doesn't model"),
            }
        };
        let (mut at, mut loaded) = (0, 0);
        loop {
            let insn = program[at];
            at += 1;
            let taken = match insn.code {
                LD_W_ABS => {
                    loaded = word(insn.k);
                    continue;
                }
                RET_K => return insn.k,
                JEQ_K => loaded == insn.k,
                JGE_K => loaded >= insn.k,
                JSET_K => loaded & insn.k != 0,
                other => panic!("unknown instruction {other:#x}"),
            };
            at += usize::from(if taken { insn.jt } else { insn.jf });
        }
    }

    fn x86_64(program: &[Insn], nr: u32, args: [u64; 2]) -> u32 {
        answer(program, AUDIT_ARCH_X86_64, nr, args)
    }

    fn i386(program: &[Insn], nr: u32, args: [u64; 2]) -> u32 {
        answer(program, AUDIT_ARCH_I386, nr, args)
    }

    /// Calls a game makes go through; namespaces, mounts and the rest are refused, with `EPERM`,
    /// or `ENOSYS` where a fallback is wanted; and `clone`, `ioctl` and `socket` are judged by
    /// their arguments — on x86-64 and i386 alike, by each one's numbers. x32's calls are refused,
    /// and any other way of calling the kernel kills the process.
    #[test]
    fn the_filter_refuses_what_a_sandboxed_game_has_no_use_for() {
        let program = program("x86_64").unwrap();
        for (nr, args) in [(0, [0, 0]), (1, [1, 0]), (9, [0, 0]), (202, [0, 0]), (231, [0, 0])] {
            assert_eq!(x86_64(&program, nr, args), ALLOW, "read, write, mmap, futex, exit_group: {nr}");
        }
        for (name, nr, answer) in X86_64.refused {
            assert_eq!(x86_64(&program, *nr, [0, 0]), *answer, "{name}");
        }
        for (name, nr, answer) in I386.refused {
            assert_eq!(i386(&program, *nr, [0, 0]), *answer, "{name} (i386)");
        }
        assert_eq!(x86_64(&program, 435, [0, 0]), errno(ENOSYS), "clone3, so that clone is used instead");

        let thread_flags = 0x003d_0f00; // what glibc's pthread_create passes
        assert_eq!(x86_64(&program, X86_64.clone, [thread_flags, 0]), ALLOW);
        assert_eq!(x86_64(&program, X86_64.clone, [u64::from(CLONE_NEWUSER) | 0x11, 0]), errno(EPERM));
        assert_eq!(i386(&program, I386.clone, [u64::from(CLONE_NEWUSER), 0]), errno(EPERM));
        assert_eq!(i386(&program, I386.clone, [thread_flags, 0]), ALLOW);

        assert_eq!(x86_64(&program, X86_64.ioctl, [0, 0x5401]), ALLOW, "TCGETS");
        assert_eq!(x86_64(&program, X86_64.ioctl, [0, u64::from(TIOCSTI)]), errno(EPERM));
        assert_eq!(x86_64(&program, X86_64.ioctl, [0, u64::from(TIOCSTI) | 0x1_0000_0000]), errno(EPERM), "the request's lower half is what counts");
        assert_eq!(i386(&program, I386.ioctl, [0, u64::from(TIOCLINUX)]), errno(EPERM));

        for family in FAMILIES {
            assert_eq!(x86_64(&program, X86_64.socket, [u64::from(family), 0]), ALLOW, "family {family}");
            assert_eq!(i386(&program, I386.socket, [u64::from(family), 0]), ALLOW, "family {family} (i386)");
        }
        for family in [17, 31, 38, 40, 21] {
            assert_eq!(x86_64(&program, X86_64.socket, [family, 0]), errno(EAFNOSUPPORT), "packet, bluetooth, alg, vsock, rds: {family}");
        }
        assert_eq!(i386(&program, 102, [1, 0]), ALLOW, "socketcall's family is out of sight, as under Flatpak");

        assert_eq!(x86_64(&program, X32_CALL, [0, 0]), errno(ENOSYS), "x32's read");
        assert_eq!(x86_64(&program, X32_CALL | 272, [0, 0]), errno(ENOSYS), "x32's unshare");
        assert_eq!(answer(&program, 0xC000_00B7, 0, [0, 0]), KILL_PROCESS, "no other way of calling exists here");
        assert_eq!(super::program("aarch64"), None);
    }

    /// Every jump lands inside the program, and every way through it ends in an answer.
    #[test]
    fn every_jump_lands_inside_the_program() {
        let program = program("x86_64").unwrap();
        for (at, insn) in program.iter().enumerate() {
            if matches!(insn.code, JEQ_K | JGE_K | JSET_K) {
                assert!(at + 1 + usize::from(insn.jt.max(insn.jf)) < program.len(), "{at}: {insn:?}");
            }
        }
        assert_eq!(program.last().map(|insn| insn.code), Some(RET_K));
    }

    /// The numbers are the kernel's own, read from its headers where they're installed (the
    /// `linux-api-headers`/`linux-libc-dev` package); skipped, saying so, where they aren't.
    #[test]
    fn the_numbers_are_the_kernels() {
        for (header, calls, extra) in [
            ("/usr/include/asm/unistd_64.h", &X86_64, [("clone", X86_64.clone), ("ioctl", X86_64.ioctl), ("socket", X86_64.socket)]),
            ("/usr/include/asm/unistd_32.h", &I386, [("clone", I386.clone), ("ioctl", I386.ioctl), ("socket", I386.socket)]),
        ] {
            let Ok(text) = std::fs::read_to_string(header) else {
                eprintln!("skipped: no {header} to check the numbers against");
                continue;
            };
            let number = |name: &str| -> Option<u32> {
                text.lines().find_map(|line| {
                    let mut words = line.split_whitespace();
                    (words.next() == Some("#define") && words.next() == Some(&format!("__NR_{name}"))).then(|| words.next()?.parse().ok()).flatten()
                })
            };
            for (name, nr) in calls.refused.iter().map(|(name, nr, _)| (*name, *nr)).chain(extra) {
                assert_eq!(number(name), Some(nr), "{name} in {header}");
            }
        }
    }
}
