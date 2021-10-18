extern crate getopts;
extern crate rand;

use getopts::Options;
use std::collections::HashMap;
use std::env;
use std::net::UdpSocket;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

const TIMEOUT: u64 = 10_000; // 10 seconds
static mut DEBUG: bool = false;

fn print_usage(program: &str, opts: Options) {
    let program_path = std::path::PathBuf::from(program);
    let program_name = program_path.file_stem().unwrap().to_str().unwrap();
    let brief = format!(
        "Usage: {} [-b BIND_ADDR] -l LOCAL_PORT -h REMOTE_ADDR -r REMOTE_PORT",
        program_name
    );
    print!("{}", opts.usage(&brief));
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.reqopt(
        "l",
        "local-port",
        "The local port to which udpproxy should bind to",
        "LOCAL_PORT",
    );
    opts.reqopt(
        "r",
        "remote-port",
        "The remote port to which UDP packets should be forwarded",
        "REMOTE_PORT",
    );
    opts.reqopt(
        "h",
        "host",
        "The remote address to which packets will be forwarded",
        "REMOTE_ADDR",
    );
    opts.optopt(
        "b",
        "bind",
        "The address on which to listen for incoming requests",
        "BIND_ADDR",
    );
    opts.optflag("d", "debug", "Enable debug mode");

    let matches = opts.parse(&args[1..]).unwrap_or_else(|_| {
        print_usage(&program, opts);
        std::process::exit(-1);
    });

    unsafe {
        DEBUG = matches.opt_present("d");
    }
    let local_port: i32 = matches.opt_str("l").unwrap().parse().unwrap();
    let remote_port: i32 = matches.opt_str("r").unwrap().parse().unwrap();
    let remote_host = matches.opt_str("h").unwrap();
    let bind_addr = match matches.opt_str("b") {
        Some(addr) => addr,
        None => "0.0.0.0".to_owned(),
    };

    forward(&bind_addr, local_port, &remote_host, remote_port);
}

fn debug(msg: String) {
    let debug: bool;
    unsafe {
        debug = DEBUG;
    }

    if debug {
        println!("{}", msg);
    }
}

type MainSender = Sender<(SocketAddr, Vec<u8>)>;
fn create_main_sender(local: &UdpSocket) -> MainSender {
    let responder = local.try_clone().expect(&format!(
        "Failed to clone primary listening address socket {}",
        local.local_addr().unwrap()
    ));

    let (main_sender, main_receiver) = channel::<(_, Vec<u8>)>();

    thread::spawn(move || {
        debug(format!(
            "Started new thread to deal out responses to clients"
        ));

        loop {
            let (dest, buf) = main_receiver.recv().unwrap();

            responder.send_to(&buf, dest).expect(&format!(
                "Failed to forward response from upstream server to client {}",
                dest
            ));
        }
    });

    main_sender
}

enum UdpProxError {
    ClientBindFail(String),
    ClientInitFail,
}

type ClientSender = Sender<Vec<u8>>;
fn create_client_sender(
    main_sender: &MainSender,
    close_queue: &Arc<RwLock<Vec<SocketAddr>>>,
    src_addr: SocketAddr,
    remote_addr: &String,
) -> Result<ClientSender, UdpProxError> {
    let main_sender = main_sender.clone();
    let close_queue = close_queue.clone();
    let (sender, receiver) = channel::<Vec<u8>>();

    let remote_addr_copy = remote_addr.clone();

    let temp_outgoing_addr = "0.0.0.0:0";

    let client_sock = if let Ok(send) = UdpSocket::bind(&temp_outgoing_addr) {
        println!("client {} -> {}", src_addr, send.local_addr().unwrap());
        send
    } else {
        eprintln!("Failed binding for client {}", src_addr);
        return Err(UdpProxError::ClientBindFail(src_addr.to_string()));
    };

    let recv_sock = if let Ok(recv) = client_sock.try_clone() {
        recv
    } else {
        eprintln!("Failed cloning upstream send for client {}", src_addr);
        return Err(UdpProxError::ClientInitFail);
    };

    // configure the receiver
    recv_sock
        .set_read_timeout(Some(Duration::from_millis(TIMEOUT + 100)))
        .unwrap();

    let mut timeouts: u64 = 0;
    let is_connected = Arc::new(AtomicBool::new(true));
    let is_connected_clone = is_connected.clone();

    thread::spawn(move || {
        // thread for receiving from target server to fake client
        thread::spawn(move || {
            let mut buf = [0; 64 * 1024];

            loop {
                match recv_sock.recv_from(&mut buf) {
                    Ok((bytes_rcvd, _)) => {
                        let to_send = buf[..bytes_rcvd].to_vec();
                        if main_sender.send((src_addr, to_send)).is_err() {
                            eprintln!(
                                "Failed to queue response from upstream server for forwarding!"
                            );
                            is_connected_clone.store(false, Ordering::Relaxed);
                            if let Ok(mut queue) = close_queue.write() {
                                queue.push(src_addr);
                            }
                            break;
                        }
                    }
                    Err(_) => {
                        if !is_connected_clone.load(Ordering::Relaxed) {
                            if let Ok(mut queue) = close_queue.write() {
                                queue.push(src_addr);
                            }
                            break;
                        }
                    }
                };
            }
        });

        // listen for messages from the client
        loop {
            if !is_connected.load(Ordering::Relaxed) {
                break;
            }

            match receiver.recv_timeout(Duration::from_millis(TIMEOUT)) {
                Ok(from_client) => {
                    if client_sock
                        .send_to(from_client.as_slice(), &remote_addr_copy)
                        .is_err()
                    {
                        eprintln!(
                            "Failed to forward packet from client {} to upstream server!",
                            src_addr
                        );
                    } else {
                        timeouts = 0; //reset timeout count
                    }
                }
                Err(_) => {
                    timeouts += 1;
                    if timeouts >= 6 {
                        debug(format!(
                            "Disconnecting forwarder for client {} due to timeout",
                            src_addr
                        ));
                        is_connected.store(false, Ordering::Relaxed);
                        break;
                    }
                }
            };
        }
    });

    Ok(sender)
}

fn forward(bind_addr: &str, local_port: i32, remote_host: &str, remote_port: i32) {
    let listen_addr = format!("{}:{}", bind_addr, local_port);
    let listen_sock = UdpSocket::bind(&listen_addr).expect(&format!(
        "Unable to bind to {}. Run `ps -ef | grep udpprox` to check if another process is running.",
        &listen_addr
    ));

    println!("Listening on {}", listen_sock.local_addr().unwrap());

    let remote_addr = format!("{}:{}", remote_host, remote_port);
    let main_sender = create_main_sender(&listen_sock);
    let mut clients: HashMap<SocketAddr, ClientSender> = HashMap::new();

    let mut buf = [0; 64 * 1024];

    let invalid_client: (usize, SocketAddr) = (
        0,
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0)),
    );

    let remove_queue = Arc::new(RwLock::new(Vec::new()));

    loop {
        // remove all clients that should be removed
        if let Ok(mut list) = remove_queue.write() {
            while let Some(addr) = list.pop() {
                println!("client {} -> closed", addr);
                clients.remove(&addr);
            }
        }

        let (num_bytes, client_addr) = listen_sock.recv_from(&mut buf).unwrap_or(invalid_client);

        if client_addr.port() == 0 {
            continue;
        }

        //we create a new thread for each unique client
        let mut remove_existing = false;

        loop {
            debug(format!("Received packet from client {}", client_addr));

            let mut is_new_client = false;

            if remove_existing {
                debug("Removing existing forwarder from map.".to_string());
                clients.remove(&client_addr);
            }

            // add a new client if the address isn't handled
            if !clients.contains_key(&client_addr) {
                if let Ok(sender) =
                    create_client_sender(&main_sender, &remove_queue, client_addr, &remote_addr)
                {
                    is_new_client = true;
                    clients.insert(client_addr, sender);
                } else {
                    break;
                }
            }

            let sender = clients.get(&client_addr).unwrap();

            let to_send = buf[..num_bytes].to_vec();
            match sender.send(to_send) {
                Ok(_) => break,
                Err(_) => {
                    if is_new_client {
                        panic!(
                            "Failed to send message to datagram forwarder for client {}",
                            client_addr
                        );
                    }

                    //client previously timed out
                    debug(format!(
                        "New connection received from previously timed-out client {}",
                        client_addr
                    ));
                    remove_existing = true;
                    continue;
                }
            }
        }
    }
}
