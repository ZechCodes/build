//! Read-only, bounded neighbor hints; callers retain subnet authorization.

use std::collections::HashSet;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::{
    io,
    net::Ipv4Addr,
    time::{Duration, Instant},
};

const HEADER_LEN: usize = 16;
const NEIGHBOR_LEN: usize = 12;
const SEQUENCE: u32 = 1;
const MAX_BYTES: usize = 256 * 1024;
const MAX_MESSAGES: usize = 4096;
const MAX_ADDRESSES: usize = 1024;
const MAX_DURATION: Duration = Duration::from_millis(5);
const DATAGRAM_BYTES: usize = 16 * 1024;

/// Consume only immediately available RTM_GETNEIGH replies. Incomplete dumps
/// are discarded; hints never authorize targets outside the caller's subnet.
pub(super) fn snapshot(interface_index: u32) -> io::Result<Vec<Ipv4Addr>> {
    if interface_index == 0 || interface_index > i32::MAX as u32 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let deadline = Instant::now() + MAX_DURATION;
    let socket = open_socket()?;
    send_request(&socket, interface_index)?;
    let mut dump = Dump::new();
    dump.deadline = Some(deadline);
    let mut buffer = [0u8; DATAGRAM_BYTES];
    loop {
        dump.check_deadline()?;
        let count = receive(&socket, &mut buffer)?;
        if dump.consume(&buffer[..count], interface_index)? {
            return Ok(dump.addresses);
        }
    }
}

fn open_socket() -> io::Result<OwnedFd> {
    // SAFETY: these are Linux netlink constants; the returned descriptor is
    // checked and transferred exactly once to OwnedFd for every return path.
    let descriptor = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let socket = unsafe { OwnedFd::from_raw_fd(descriptor) };
    // SAFETY: zero initializes sockaddr_nl including its private padding. A
    // zero port requests a unique kernel-assigned ID; no groups are joined.
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as u16;
    let result = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            (&address as *const libc::sockaddr_nl).cast(),
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket)
}

fn request(interface_index: u32) -> [u8; HEADER_LEN + NEIGHBOR_LEN] {
    let mut request = [0u8; HEADER_LEN + NEIGHBOR_LEN];
    request[..4].copy_from_slice(&((HEADER_LEN + NEIGHBOR_LEN) as u32).to_ne_bytes());
    request[4..6].copy_from_slice(&libc::RTM_GETNEIGH.to_ne_bytes());
    request[6..8].copy_from_slice(&((libc::NLM_F_REQUEST | libc::NLM_F_DUMP) as u16).to_ne_bytes());
    request[8..12].copy_from_slice(&SEQUENCE.to_ne_bytes());
    request[HEADER_LEN] = libc::AF_INET as u8;
    request[20..24].copy_from_slice(&interface_index.to_ne_bytes());
    request
}

fn send_request(socket: &OwnedFd, interface_index: u32) -> io::Result<()> {
    let request = request(interface_index);
    // SAFETY: sockaddr_nl and the fixed request are initialized for the whole
    // call. Port/group zero address the kernel only; GET never mutates entries.
    let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    kernel.nl_family = libc::AF_NETLINK as u16;
    let count = unsafe {
        libc::sendto(
            socket.as_raw_fd(),
            request.as_ptr().cast(),
            request.len(),
            libc::MSG_DONTWAIT,
            (&kernel as *const libc::sockaddr_nl).cast(),
            std::mem::size_of_val(&kernel) as libc::socklen_t,
        )
    };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    if count as usize != request.len() {
        return Err(io::ErrorKind::WriteZero.into());
    }
    Ok(())
}

fn receive(socket: &OwnedFd, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: recvmsg receives into initialized sockaddr storage and exactly
    // buffer.len() writable bytes; no returned length is trusted until checked.
    let mut sender: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    let mut vector = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_name = (&mut sender as *mut libc::sockaddr_nl).cast();
    message.msg_namelen = std::mem::size_of_val(&sender) as libc::socklen_t;
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    let count = unsafe {
        libc::recvmsg(
            socket.as_raw_fd(),
            &mut message,
            libc::MSG_DONTWAIT | libc::MSG_TRUNC,
        )
    };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    validate_sender(&sender, message.msg_namelen)?;
    if count == 0 || count as usize > buffer.len() || message.msg_flags & libc::MSG_TRUNC != 0 {
        return Err(invalid_dump());
    }
    Ok(count as usize)
}

fn validate_sender(sender: &libc::sockaddr_nl, length: libc::socklen_t) -> io::Result<()> {
    if length as usize != std::mem::size_of_val(sender)
        || sender.nl_family != libc::AF_NETLINK as u16
        || sender.nl_pid != 0
        || sender.nl_groups != 0
    {
        return Err(invalid_dump());
    }
    Ok(())
}

struct Dump {
    addresses: Vec<Ipv4Addr>,
    seen: HashSet<Ipv4Addr>,
    bytes: usize,
    messages: usize,
    deadline: Option<Instant>,
}

impl Dump {
    fn new() -> Self {
        Self {
            addresses: Vec::new(),
            seen: HashSet::new(),
            bytes: 0,
            messages: 0,
            deadline: None,
        }
    }

    fn check_deadline(&self) -> io::Result<()> {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }

    fn consume(&mut self, mut data: &[u8], interface_index: u32) -> io::Result<bool> {
        self.check_deadline()?;
        self.bytes = self
            .bytes
            .checked_add(data.len())
            .ok_or_else(invalid_dump)?;
        if self.bytes > MAX_BYTES {
            return Err(invalid_dump());
        }
        let mut done = false;
        while !data.is_empty() {
            self.check_deadline()?;
            self.messages += 1;
            if self.messages > MAX_MESSAGES || data.len() < HEADER_LEN {
                return Err(invalid_dump());
            }
            let length = u32::from_ne_bytes(data[..4].try_into().unwrap()) as usize;
            if length < HEADER_LEN || length > data.len() {
                return Err(invalid_dump());
            }
            let sequence = u32::from_ne_bytes(data[8..12].try_into().unwrap());
            if sequence == SEQUENCE {
                let kind = u16::from_ne_bytes(data[4..6].try_into().unwrap());
                let flags = u16::from_ne_bytes(data[6..8].try_into().unwrap());
                if flags & libc::NLM_F_DUMP_INTR as u16 != 0 {
                    return Err(invalid_dump());
                }
                done |= self.message(kind, &data[HEADER_LEN..length], interface_index)?;
            }
            data = advance(data, length)?;
        }
        Ok(done)
    }

    fn message(&mut self, kind: u16, payload: &[u8], interface_index: u32) -> io::Result<bool> {
        match kind as i32 {
            libc::NLMSG_DONE => {
                status(payload, false)?;
                Ok(true)
            }
            libc::NLMSG_ERROR => {
                status(payload, true)?;
                Ok(false)
            }
            libc::NLMSG_OVERRUN => Err(invalid_dump()),
            _ if kind == libc::RTM_NEWNEIGH => {
                self.neighbor(payload, interface_index)?;
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn neighbor(&mut self, payload: &[u8], interface_index: u32) -> io::Result<()> {
        if payload.len() < NEIGHBOR_LEN {
            return Err(invalid_dump());
        }
        let owner = i32::from_ne_bytes(payload[4..8].try_into().unwrap());
        let state = u16::from_ne_bytes(payload[8..10].try_into().unwrap());
        if payload[0] != libc::AF_INET as u8
            || owner <= 0
            || owner as u32 != interface_index
            || !matches!(
                state,
                libc::NUD_REACHABLE | libc::NUD_STALE | libc::NUD_DELAY
            )
        {
            return Ok(());
        }
        if let Some(address) = destination(&payload[NEIGHBOR_LEN..])? {
            if self.seen.contains(&address) {
                return Ok(());
            }
            if self.addresses.len() == MAX_ADDRESSES {
                return Err(invalid_dump());
            }
            self.seen.insert(address);
            self.addresses.push(address);
        }
        Ok(())
    }
}

fn destination(mut data: &[u8]) -> io::Result<Option<Ipv4Addr>> {
    let mut address = None;
    while !data.is_empty() {
        if data.len() < 4 {
            return Err(invalid_dump());
        }
        let length = u16::from_ne_bytes(data[..2].try_into().unwrap()) as usize;
        let kind = u16::from_ne_bytes(data[2..4].try_into().unwrap());
        if length < 4 || length > data.len() {
            return Err(invalid_dump());
        }
        if kind == libc::NDA_DST {
            if length != 8 || address.is_some() {
                return Err(invalid_dump());
            }
            address = Some(Ipv4Addr::new(data[4], data[5], data[6], data[7]));
        }
        data = advance(data, length)?;
    }
    Ok(address)
}

fn status(payload: &[u8], acknowledgement: bool) -> io::Result<()> {
    if payload.is_empty() && !acknowledgement {
        return Ok(());
    }
    if payload.len() < if acknowledgement { 20 } else { 4 } {
        return Err(invalid_dump());
    }
    let value = i32::from_ne_bytes(payload[..4].try_into().unwrap());
    if value == 0 {
        return Ok(());
    }
    if value > 0 {
        return Err(invalid_dump());
    }
    Err(io::Error::from_raw_os_error(
        value.checked_neg().ok_or_else(invalid_dump)?,
    ))
}

fn advance(data: &[u8], length: usize) -> io::Result<&[u8]> {
    let aligned = length.checked_add(3).ok_or_else(invalid_dump)? & !3;
    if aligned <= data.len() {
        return Ok(&data[aligned..]);
    }
    if length == data.len() {
        return Ok(&[]);
    }
    Err(invalid_dump())
}

fn invalid_dump() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "incomplete or oversized neighbor dump",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: u32 = 7;

    fn message(kind: u16, flags: u16, sequence: u32, payload: &[u8]) -> Vec<u8> {
        let length = HEADER_LEN + payload.len();
        let mut result = Vec::new();
        result.extend_from_slice(&(length as u32).to_ne_bytes());
        result.extend_from_slice(&kind.to_ne_bytes());
        result.extend_from_slice(&flags.to_ne_bytes());
        result.extend_from_slice(&sequence.to_ne_bytes());
        result.extend_from_slice(&0u32.to_ne_bytes());
        result.extend_from_slice(payload);
        result.resize((length + 3) & !3, 0);
        result
    }

    fn attribute(kind: u16, value: &[u8]) -> Vec<u8> {
        let length = 4 + value.len();
        let mut result = Vec::new();
        result.extend_from_slice(&(length as u16).to_ne_bytes());
        result.extend_from_slice(&kind.to_ne_bytes());
        result.extend_from_slice(value);
        result.resize((length + 3) & !3, 0);
        result
    }

    fn neighbor(family: u8, index: u32, state: u16, address: Ipv4Addr) -> Vec<u8> {
        let mut payload = vec![family, 0, 0, 0];
        payload.extend_from_slice(&index.to_ne_bytes());
        payload.extend_from_slice(&state.to_ne_bytes());
        payload.extend_from_slice(&[0, 0]);
        payload.extend_from_slice(&attribute(1, &address.octets()));
        message(28, 2, SEQUENCE, &payload)
    }

    fn consume(data: &[u8]) -> io::Result<Dump> {
        let mut dump = Dump::new();
        dump.consume(data, OWNER)?;
        Ok(dump)
    }

    #[test]
    fn only_exact_ipv4_owner_and_three_dynamic_states_are_hints() {
        let mut data = Vec::new();
        for (family, owner, state, octet) in [
            (2, OWNER, 2, 1),
            (2, OWNER, 4, 2),
            (2, OWNER, 8, 3),
            (10, OWNER, 2, 4),
            (2, OWNER + 1, 2, 5),
            (2, OWNER, 0, 6),
            (2, OWNER, 1, 7),
            (2, OWNER, 16, 8),
            (2, OWNER, 32, 9),
            (2, OWNER, 64, 10),
            (2, OWNER, 128, 11),
            (2, OWNER, 2 | 32, 12),
        ] {
            data.extend(neighbor(
                family,
                owner,
                state,
                Ipv4Addr::new(10, 1, 2, octet),
            ));
        }
        data.extend(neighbor(2, OWNER, 4, Ipv4Addr::new(10, 1, 2, 1)));
        assert_eq!(
            consume(&data).unwrap().addresses,
            vec![
                Ipv4Addr::new(10, 1, 2, 1),
                Ipv4Addr::new(10, 1, 2, 2),
                Ipv4Addr::new(10, 1, 2, 3),
            ]
        );
    }

    #[test]
    fn multipart_ignores_other_sequences_and_message_types() {
        let mut dump = Dump::new();
        let mut first = neighbor(2, OWNER, 2, Ipv4Addr::new(192, 168, 1, 9));
        first[8..12].copy_from_slice(&2u32.to_ne_bytes());
        first.extend(message(16, 2, SEQUENCE, &[]));
        assert!(!dump.consume(&first, OWNER).unwrap());
        assert!(dump.addresses.is_empty());
        let mut second = neighbor(2, OWNER, 2, Ipv4Addr::new(192, 168, 1, 10));
        second.extend(message(3, 2, SEQUENCE, &0i32.to_ne_bytes()));
        assert!(dump.consume(&second, OWNER).unwrap());
        assert_eq!(dump.addresses, vec![Ipv4Addr::new(192, 168, 1, 10)]);
    }

    #[test]
    fn aligned_unknown_attributes_are_skipped_without_reinterpreting_bytes() {
        let mut payload = vec![2, 0, 0, 0];
        payload.extend_from_slice(&OWNER.to_ne_bytes());
        payload.extend_from_slice(&2u16.to_ne_bytes());
        payload.extend_from_slice(&[0, 0]);
        payload.extend(attribute(2, &[1, 2, 3, 4, 5, 6]));
        payload.extend(attribute(1, &[172, 19, 4, 200]));
        assert_eq!(
            consume(&message(28, 2, SEQUENCE, &payload))
                .unwrap()
                .addresses,
            vec![Ipv4Addr::new(172, 19, 4, 200)]
        );
    }

    #[test]
    fn truncated_or_malformed_headers_and_attributes_are_errors() {
        let valid = neighbor(2, OWNER, 2, Ipv4Addr::new(10, 0, 0, 1));
        for length in [
            1,
            HEADER_LEN - 1,
            HEADER_LEN + NEIGHBOR_LEN - 1,
            valid.len() - 1,
        ] {
            assert_eq!(
                consume(&valid[..length]).err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
        }
        for malformed_length in [0u32, 15, u32::MAX] {
            let mut data = valid.clone();
            data[..4].copy_from_slice(&malformed_length.to_ne_bytes());
            assert_eq!(
                consume(&data).err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
        }
        for malformed_length in [0u16, 3, 7, 9] {
            let mut data = valid.clone();
            data[28..30].copy_from_slice(&malformed_length.to_ne_bytes());
            assert_eq!(
                consume(&data).err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        assert_eq!(
            consume(&trailing).err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        let mut duplicate = valid[HEADER_LEN..].to_vec();
        duplicate.extend(attribute(1, &[10, 0, 0, 2]));
        assert_eq!(
            consume(&message(28, 2, SEQUENCE, &duplicate))
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn incomplete_dump_and_kernel_errors_never_return_partial_success() {
        let mut error = (-libc::EPERM).to_ne_bytes().to_vec();
        error.resize(20, 0);
        assert_eq!(
            consume(&message(2, 0, SEQUENCE, &error))
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            consume(&message(3, 0, SEQUENCE, &(-libc::EINVAL).to_ne_bytes()))
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        assert!(consume(&message(4, 0, SEQUENCE, &[])).is_err());
        assert!(consume(&message(3, 0x10, SEQUENCE, &[])).is_err());
        for (kind, payload) in [(2, vec![0; 3]), (3, vec![0; 3])] {
            assert_eq!(
                consume(&message(kind, 0, SEQUENCE, &payload))
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        let mut acknowledgement = vec![0; 20];
        assert!(
            !Dump::new()
                .consume(&message(2, 0, SEQUENCE, &acknowledgement), OWNER)
                .unwrap()
        );
        acknowledgement[..4].copy_from_slice(&i32::MIN.to_ne_bytes());
        assert!(consume(&message(2, 0, SEQUENCE, &acknowledgement)).is_err());
    }

    #[test]
    fn address_message_and_byte_budgets_are_enforced_across_datagrams() {
        let mut addresses = Dump::new();
        for value in 0..MAX_ADDRESSES {
            addresses
                .consume(
                    &neighbor(2, OWNER, 2, Ipv4Addr::from(0x0a000001 + value as u32)),
                    OWNER,
                )
                .unwrap();
        }
        assert_eq!(addresses.addresses.len(), MAX_ADDRESSES);
        assert!(
            addresses
                .consume(&neighbor(2, OWNER, 2, Ipv4Addr::new(10, 1, 0, 1)), OWNER)
                .is_err()
        );
        let mut messages = Dump::new();
        for _ in 0..MAX_MESSAGES {
            messages
                .consume(&message(1, 0, SEQUENCE, &[]), OWNER)
                .unwrap();
        }
        assert!(
            messages
                .consume(&message(1, 0, SEQUENCE, &[]), OWNER)
                .is_err()
        );
        let mut bytes = Dump::new();
        let chunk = message(16, 0, SEQUENCE, &vec![0; 4096 - HEADER_LEN]);
        for _ in 0..MAX_BYTES / chunk.len() {
            bytes.consume(&chunk, OWNER).unwrap();
        }
        assert!(bytes.consume(&message(1, 0, SEQUENCE, &[]), OWNER).is_err());
    }

    #[test]
    fn invalid_interface_indices_are_rejected_before_opening_a_socket() {
        for index in [0, u32::MAX] {
            assert_eq!(
                snapshot(index).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn an_expired_budget_rejects_even_an_immediately_available_dump() {
        let mut dump = Dump::new();
        dump.deadline = Some(Instant::now() - std::time::Duration::from_millis(1));
        assert_eq!(
            dump.consume(&message(3, 0, SEQUENCE, &[]), OWNER)
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn only_kernel_unicast_senders_are_accepted() {
        let mut sender: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sender.nl_family = libc::AF_NETLINK as u16;
        let length = std::mem::size_of_val(&sender) as libc::socklen_t;
        assert!(validate_sender(&sender, length).is_ok());
        assert!(validate_sender(&sender, length - 1).is_err());
        sender.nl_pid = 123;
        assert!(validate_sender(&sender, length).is_err());
        sender.nl_pid = 0;
        sender.nl_groups = 1;
        assert!(validate_sender(&sender, length).is_err());
        sender.nl_groups = 0;
        sender.nl_family = libc::AF_INET as u16;
        assert!(validate_sender(&sender, length).is_err());
    }

    #[test]
    fn request_is_only_an_ipv4_neighbor_dump_for_the_owner() {
        let data = request(OWNER);
        assert_eq!(
            u16::from_ne_bytes(data[4..6].try_into().unwrap()),
            libc::RTM_GETNEIGH
        );
        assert_eq!(
            u16::from_ne_bytes(data[6..8].try_into().unwrap()),
            (libc::NLM_F_REQUEST | libc::NLM_F_DUMP) as u16
        );
        assert_eq!(data[HEADER_LEN], libc::AF_INET as u8);
        assert_eq!(u32::from_ne_bytes(data[20..24].try_into().unwrap()), OWNER);
        assert!(data[24..].iter().all(|byte| *byte == 0));
    }
}
