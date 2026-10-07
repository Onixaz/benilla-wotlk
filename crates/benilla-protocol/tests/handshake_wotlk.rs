//! The 3.3.5a world handshake over a real socket against a fake server that checks
//! `CMSG_AUTH_SESSION` as AzerothCore's `WorldSocket::HandleAuthSession` does and answers with
//! RC4 headers. Its own test binary, since the target is fixed per process.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use benilla_protocol::messages::{self, opcode};
use benilla_protocol::{WorldAuthReject, WorldSession};
use benilla_srp::wotlk_header::WotlkHeaderCrypto;
use benilla_srp::SESSION_KEY_LENGTH;
use sha1::{Digest, Sha1};

const SESSION_KEY: [u8; SESSION_KEY_LENGTH] = [0x29; SESSION_KEY_LENGTH];
const SERVER_SEED: u32 = 0x1234_5678;
const REALM_ID: u8 = 1;
/// AzerothCore's `REALM_LIST_REALM_NOT_FOUND`, sent for a realm id that is not its own.
const REALM_LIST_REALM_NOT_FOUND: u8 = 0x2A;

fn wotlk() {
    std::env::set_var(benilla_expansions::EXPANSION_ENV, "wotlk");
    assert_eq!(
        benilla_expansions::Expansion::active(),
        benilla_expansions::Expansion::Wotlk
    );
}

/// One server packet: a 4-byte header, or 5 bytes past `0x7FFF`, encrypted once `crypto` is set.
fn send(stream: &mut TcpStream, crypto: Option<&mut WotlkHeaderCrypto>, op: u16, body: &[u8]) {
    let size = (body.len() + 2) as u32;
    let o = op.to_le_bytes();
    let mut header = if size > 0x7FFF {
        let s = size.to_be_bytes();
        vec![0x80 | s[1], s[2], s[3], o[0], o[1]]
    } else {
        let s = (size as u16).to_be_bytes();
        vec![s[0], s[1], o[0], o[1]]
    };
    if let Some(c) = crypto {
        c.encrypt.apply(&mut header);
    }
    stream.write_all(&header).unwrap();
    stream.write_all(body).unwrap();
}

fn read_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The fields of the client's `CMSG_AUTH_SESSION` the server acts on.
struct AuthSession {
    build: u32,
    account: String,
    realm_id: u32,
    proof_ok: bool,
    addon_count: u32,
}

fn read_auth_session(stream: &mut TcpStream) -> AuthSession {
    let mut header = [0u8; 6];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(read_u32(&header, 2), u32::from(opcode::CMSG_AUTH_SESSION));
    let size = u16::from_be_bytes([header[0], header[1]]) as usize;
    let mut b = vec![0u8; size - 4];
    stream.read_exact(&mut b).unwrap();

    let build = read_u32(&b, 0);
    let nul = 8 + b[8..].iter().position(|&c| c == 0).unwrap();
    let account = String::from_utf8(b[8..nul].to_vec()).unwrap();
    let mut at = nul + 1;
    at += 4; // login server type
    let client_seed = &b[at..at + 4];
    at += 4;
    at += 8; // region, battlegroup
    let realm_id = read_u32(&b, at);
    at += 4;
    at += 8; // DoS response
    let digest = &b[at..at + 20];
    at += 20;

    let expected: [u8; 20] = Sha1::new()
        .chain_update(account.as_bytes())
        .chain_update([0u8; 4])
        .chain_update(client_seed)
        .chain_update(SERVER_SEED.to_le_bytes())
        .chain_update(SESSION_KEY)
        .finalize()
        .into();

    let addon_size = read_u32(&b, at) as usize;
    let mut addons = Vec::new();
    flate2::read::ZlibDecoder::new(&b[at + 4..])
        .read_to_end(&mut addons)
        .unwrap();
    assert_eq!(addons.len(), addon_size);

    AuthSession {
        build,
        account,
        realm_id,
        proof_ok: digest == expected,
        addon_count: read_u32(&addons, 0),
    }
}

/// A fake 3.3.5a world server: the challenge, then the auth session checked, then `pre` and the
/// auth response, encrypted; then, if `post` is not empty, it reads one client packet and answers
/// with `post`. Returns its address and what it read.
fn fake_server(
    pre: Vec<(u16, Vec<u8>)>,
    post: Vec<(u16, Vec<u8>)>,
) -> (String, std::sync::mpsc::Receiver<AuthSession>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut challenge = 1u32.to_le_bytes().to_vec();
        challenge.extend_from_slice(&SERVER_SEED.to_le_bytes());
        challenge.extend_from_slice(&[0xAB; 32]);
        send(&mut stream, None, opcode::SMSG_AUTH_CHALLENGE, &challenge);

        let session = read_auth_session(&mut stream);
        let mut crypto = WotlkHeaderCrypto::server(&SESSION_KEY);
        if session.realm_id != u32::from(REALM_ID) {
            send(
                &mut stream,
                Some(&mut crypto),
                opcode::SMSG_AUTH_RESPONSE,
                &[REALM_LIST_REALM_NOT_FOUND],
            );
        } else {
            for (op, body) in pre {
                send(&mut stream, Some(&mut crypto), op, &body);
            }
            // AUTH_OK, billing remaining, plan flags, billing rested, expansion.
            let mut ok = vec![messages::AUTH_OK];
            ok.extend_from_slice(&0u32.to_le_bytes());
            ok.push(0);
            ok.extend_from_slice(&0u32.to_le_bytes());
            ok.push(2);
            send(
                &mut stream,
                Some(&mut crypto),
                opcode::SMSG_AUTH_RESPONSE,
                &ok,
            );
            if !post.is_empty() {
                // A client header: u16 size (big-endian), u32 opcode, through the RC4 stream.
                let mut header = [0u8; 6];
                stream.read_exact(&mut header).unwrap();
                crypto.decrypt.apply(&mut header);
                let size = u16::from_be_bytes([header[0], header[1]]) as usize;
                stream.read_exact(&mut vec![0u8; size - 4]).unwrap();
                for (op, body) in post {
                    send(&mut stream, Some(&mut crypto), op, &body);
                }
            }
        }
        tx.send(session).unwrap();
        thread::sleep(std::time::Duration::from_secs(2));
    });
    (addr, rx)
}

#[test]
fn the_server_accepts_the_auth_session_and_answers_through_rc4() {
    wotlk();
    // A large packet ahead of the response exercises the 5-byte header.
    let (addr, seen) = fake_server(
        vec![
            (opcode::SMSG_UPDATE_OBJECT, vec![0x5A; 0x9000]),
            (opcode::SMSG_TUTORIAL_FLAGS, vec![0; 32]),
        ],
        vec![],
    );
    WorldSession::connect(&addr, "one", SESSION_KEY, REALM_ID).expect("handshake");
    let session = seen.recv().unwrap();
    assert_eq!(session.build, 12340);
    assert_eq!(session.account, "ONE");
    assert_eq!(session.realm_id, 1);
    assert!(session.proof_ok, "the server's digest check passes");
    assert_eq!(session.addon_count, 23);
}

/// The roster reads in 3.3.5a's layout: a second entry lands only if the first's customize flags
/// and enchant visuals were consumed.
#[test]
fn the_roster_reads_in_the_wotlk_layout() {
    wotlk();
    let entry = |guid: u64, name: &str, race: u8| {
        let mut b = guid.to_le_bytes().to_vec();
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        b.extend_from_slice(&[race, 1, 0, 1, 2, 3, 4, 5, 70]);
        b.extend_from_slice(&[0; 4 + 4 + 12 + 4]); // zone, map, position, guild
        b.extend_from_slice(&0x2000u32.to_le_bytes()); // character flags (GHOST)
        b.extend_from_slice(&0u32.to_le_bytes()); // customize flags
        b.push(0); // first login
        b.extend_from_slice(&[0; 12]); // pet
        for slot in 0..23u32 {
            b.extend_from_slice(&(500 + slot).to_le_bytes());
            b.push(slot as u8);
            b.extend_from_slice(&7u32.to_le_bytes());
        }
        b
    };
    let mut roster = vec![2u8];
    roster.extend(entry(1, "Orc", 2));
    roster.extend(entry(2, "Draenei", 11));
    let (addr, _seen) = fake_server(
        vec![],
        vec![
            // An unparsed packet ahead of the roster is skipped.
            (opcode::SMSG_TUTORIAL_FLAGS, vec![0; 32]),
            (opcode::SMSG_CHAR_ENUM, roster),
        ],
    );
    let mut session = WorldSession::connect(&addr, "one", SESSION_KEY, REALM_ID).unwrap();
    let characters = session.char_enum().expect("roster");
    let got: Vec<_> = characters
        .iter()
        .map(|c| (c.name.as_str(), c.race, c.flags, c.equipment[18].display_id))
        .collect();
    assert_eq!(got, [("Orc", 2, 0x2000, 518), ("Draenei", 11, 0x2000, 518)]);
}

#[test]
fn a_wrong_realm_id_is_the_server_s_refusal() {
    wotlk();
    let (addr, _seen) = fake_server(vec![], vec![]);
    let Err(err) = WorldSession::connect(&addr, "one", SESSION_KEY, REALM_ID + 1) else {
        panic!("a wrong realm id must be refused");
    };
    let refusal = err
        .downcast_ref::<WorldAuthReject>()
        .expect("a world refusal");
    assert_eq!(refusal.code, REALM_LIST_REALM_NOT_FOUND);
}
