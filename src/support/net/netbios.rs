//! Asking a Windows machine its own name — a NetBIOS **node status** query (NBNS, UDP/137).
//!
//! The third and last of the LAN's name protocols, after mDNS and unicast DNS. It exists because
//! the other two miss exactly the machines most likely to be sharing files: a Windows box with no
//! Avahi and no router-registered PTR answers nothing to either, yet has always been willing to
//! say its name to anyone who asks on port 137. `nbtscan` and `nmap -sU --script nbstat` do the
//! same thing.
//!
//! # The wire format, and why it isn't DNS
//!
//! NBNS borrows DNS's *header* and nothing else useful. The question name is not a domain: it is a
//! fixed 16-byte NetBIOS name put through "first-level encoding", where each byte becomes two
//! characters — the nibbles, each added to `'A'`. The wildcard name `*` (padded with NULs) encodes
//! to `CKAAAA…`, which is why every node-status query on earth looks identical and is 50 bytes
//! long.
//!
//! The answer carries a *table* of names, because a machine has several: its own, its workgroup's,
//! and service registrations. They are told apart by a one-byte suffix and a group flag — the
//! machine's own name is the unique one with suffix `0x00`, and picking any other yields the
//! workgroup (`WORKGROUP`, on an awful lot of networks) instead of the computer.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

/// The NetBIOS name service port.
const NBNS_PORT: u16 = 137;

/// `NBSTAT`, the node-status query type.
const TYPE_NBSTAT: u16 = 0x0021;

/// Suffix of a machine's own name in the returned table. Other suffixes are services (`0x20` is
/// the file-server registration) or, with the group bit, the workgroup.
const SUFFIX_WORKSTATION: u8 = 0x00;

/// Set in a table entry's flags when the name is a GROUP (a workgroup or domain) rather than the
/// machine itself.
const FLAG_GROUP: u16 = 0x8000;

/// Ask `ip` what it calls itself. `None` when it doesn't answer — which is every machine that
/// isn't running NetBIOS, i.e. most of them.
pub(crate) fn name(ip: Ipv4Addr, timeout: Duration) -> Option<String> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.set_read_timeout(Some(timeout)).ok()?;
    socket.send_to(&node_status_query(), SocketAddr::from((ip, NBNS_PORT))).ok()?;
    let mut buffer = [0u8; 1024];
    let (read, from) = socket.recv_from(&mut buffer).ok()?;
    // Only the machine we asked may answer for itself; a reply from anywhere else is not about
    // this address.
    (from.ip() == ip).then_some(())?;
    parse_node_status(&buffer[..read])
}

/// The 50-byte node-status query. Identical on every network — the only variable is the
/// transaction id, and nothing here depends on matching it, since the socket is bound to an
/// ephemeral port that receives only the reply to this datagram.
fn node_status_query() -> Vec<u8> {
    let mut query = Vec::with_capacity(50);
    query.extend_from_slice(&[0x00, 0x00]); // transaction id
    query.extend_from_slice(&[0x00, 0x00]); // flags: a plain query
    query.extend_from_slice(&[0x00, 0x01]); // one question
    query.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // no answer/authority/additional records
    query.push(0x20); // the encoded name is always 32 bytes
    query.extend_from_slice(&encode_netbios_name(b"*"));
    query.push(0x00); // root label
    query.extend_from_slice(&TYPE_NBSTAT.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes()); // class IN
    query
}

/// NetBIOS "first-level encoding": pad `name` to 16 bytes with NULs, then split every byte into
/// its two nibbles and add `'A'` to each. Sixteen bytes in, thirty-two characters out.
fn encode_netbios_name(name: &[u8]) -> [u8; 32] {
    let mut padded = [0u8; 16];
    for (slot, byte) in padded.iter_mut().zip(name) {
        *slot = *byte;
    }
    let mut encoded = [0u8; 32];
    for (index, byte) in padded.iter().enumerate() {
        encoded[index * 2] = b'A' + (byte >> 4);
        encoded[index * 2 + 1] = b'A' + (byte & 0x0F);
    }
    encoded
}

/// Pull the machine's own name out of a node-status reply.
///
/// Split from the socket work so the table walk — which is where the interesting mistakes live —
/// is testable against captured bytes rather than requiring a Windows machine on the network.
pub(crate) fn parse_node_status(reply: &[u8]) -> Option<String> {
    // Header (12) + the echoed question name (1 + 32 + 1) + type (2) + class (2) + ttl (4)
    // + rdlength (2) = 56, then the record data begins with the number of names.
    const RDATA_AT: usize = 56;
    let answers = u16::from_be_bytes([*reply.get(6)?, *reply.get(7)?]);
    (answers > 0).then_some(())?;
    let count = *reply.get(RDATA_AT)? as usize;
    // Each table entry is exactly 18 bytes: 15 of name, one suffix, two of flags. `as_chunks`
    // discards any trailing partial entry, which is what keeps a truncated datagram harmless.
    let (entries, _) = reply.get(RDATA_AT + 1..)?.as_chunks::<18>();
    entries.iter().take(count).find_map(|entry| {
        let flags = u16::from_be_bytes([entry[16], entry[17]]);
        // The machine's own name: suffix 0x00 and NOT a group. Skipping either test yields the
        // workgroup instead, which is the same on every machine in the building.
        (entry[15] == SUFFIX_WORKSTATION && flags & FLAG_GROUP == 0).then_some(())?;
        let text = String::from_utf8_lossy(&entry[..15]).trim().to_string();
        (!text.is_empty() && text.chars().all(|c| !c.is_control())).then_some(text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a node-status reply carrying `names` as `(name, suffix, group)`.
    fn reply(names: &[(&str, u8, bool)]) -> Vec<u8> {
        let mut out = vec![0x00, 0x00, 0x84, 0x00]; // id, flags (a response)
        out.extend_from_slice(&[0x00, 0x00]); // questions
        out.extend_from_slice(&1u16.to_be_bytes()); // ONE answer
        out.extend_from_slice(&[0, 0, 0, 0]); // authority / additional
        out.push(0x20);
        out.extend_from_slice(&encode_netbios_name(b"*"));
        out.push(0x00);
        out.extend_from_slice(&TYPE_NBSTAT.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes()); // class
        out.extend_from_slice(&0u32.to_be_bytes()); // ttl
        out.extend_from_slice(&0u16.to_be_bytes()); // rdlength (unread)
        assert_eq!(out.len(), 56, "the header+question prefix must be the documented 56 bytes");
        out.push(names.len() as u8);
        for (name, suffix, group) in names {
            let mut padded = [b' '; 15];
            for (slot, byte) in padded.iter_mut().zip(name.as_bytes()) {
                *slot = *byte;
            }
            out.extend_from_slice(&padded);
            out.push(*suffix);
            out.extend_from_slice(&(if *group { FLAG_GROUP } else { 0x0400u16 }).to_be_bytes());
        }
        out
    }

    /// The query is a fixed 50 bytes, and the encoded wildcard is the reason every node-status
    /// probe on earth looks the same. Getting the encoding wrong yields a packet nothing answers.
    #[test]
    fn the_node_status_query_is_the_canonical_fifty_bytes() {
        let query = node_status_query();
        assert_eq!(query.len(), 50, "the standard node-status query length");
        assert_eq!(&query[4..6], &[0, 1], "exactly one question");
        assert_eq!(query[12], 0x20, "the encoded name is 32 bytes");
        assert_eq!(&query[13..45], b"CKAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "the encoded `*` wildcard");
        assert_eq!(query[45], 0x00, "root label");
        assert_eq!(&query[46..48], &[0x00, 0x21], "type NBSTAT");
        assert_eq!(&query[48..50], &[0x00, 0x01], "class IN");
    }

    /// `*` is 0x2A: nibbles 2 and A, so 'A'+2='C' and 'A'+10='K'. Every NUL becomes "AA".
    #[test]
    fn netbios_encoding_splits_each_byte_into_two_letters() {
        assert_eq!(&encode_netbios_name(b"*")[..4], b"CKAA");
        // A real name encodes the same way, and always to exactly 32 characters.
        let encoded = encode_netbios_name(b"PC");
        assert_eq!(encoded.len(), 32);
        assert_eq!(&encoded[..4], b"FAED", "'P'=0x50 -> nibbles 5,0 -> FA; 'C'=0x43 -> 4,3 -> ED");
        assert!(encoded.iter().all(|byte| (b'A'..=b'P').contains(byte)), "nibbles land in A..P");
    }

    /// The whole point of the suffix and group checks: a machine returns several names, and the
    /// wrong pick gives the workgroup — which is identical on every machine in the building.
    #[test]
    fn the_machines_own_name_is_taken_over_its_workgroup_and_services() {
        let bytes = reply(&[
            ("WORKGROUP", 0x00, true),   // the workgroup, sharing the 0x00 suffix — a real trap
            ("DESKTOP-7F2", 0x00, false), // the machine itself
            ("DESKTOP-7F2", 0x20, false), // its file-server registration
        ]);
        assert_eq!(parse_node_status(&bytes).as_deref(), Some("DESKTOP-7F2"));

        // With only a group name, there is no machine name to report — better nothing than
        // labelling every Windows box "WORKGROUP".
        assert_eq!(parse_node_status(&reply(&[("WORKGROUP", 0x00, true)])), None);
        // A service registration alone is not the machine's name either.
        assert_eq!(parse_node_status(&reply(&[("FILESERVER", 0x20, false)])), None);
    }

    #[test]
    fn a_truncated_or_empty_reply_yields_nothing_rather_than_panicking() {
        assert_eq!(parse_node_status(&[]), None);
        assert_eq!(parse_node_status(&[0; 12]), None, "a header claiming no answers");
        let good = reply(&[("PC", 0x00, false)]);
        // Every truncation of a valid reply must be survivable — a hostile or damaged datagram
        // arrives on the same socket as a good one.
        for cut in 0..good.len() {
            let _ = parse_node_status(&good[..cut]);
        }
        // A count claiming more entries than the datagram holds must not read past the end.
        let mut lying = reply(&[("PC", 0x00, false)]);
        lying[56] = 200;
        assert_eq!(parse_node_status(&lying).as_deref(), Some("PC"), "reads only what is there");
    }

    #[test]
    fn names_are_trimmed_of_their_padding() {
        assert_eq!(parse_node_status(&reply(&[("NAS", 0x00, false)])).as_deref(), Some("NAS"));
    }
}
