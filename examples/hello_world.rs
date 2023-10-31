use takyon::{
    time::sleep_secs,
    net::UdpSocket
};

pub fn main() {
    takyon::init().unwrap();

    takyon::run(async {
        takyon::spawn(async {
            let mut i = 0;

            loop {
                sleep_secs(1).await;
                println!("Tick {i}");
                i += 1;
            }
        });

        let sock = UdpSocket::bind("127.0.0.1:5000").unwrap();
        let mut buf = [0; 2048];

        loop {
            let (bytes, src_addr) = sock.recv_from(&mut buf).await.unwrap();

            println!("Read {:?} bytes from address {:?}", bytes, src_addr);
            println!("Data: {:02X?}\n", &buf[..bytes]);

            if &buf[..bytes] == &[0xB0, 0x0B] {
                break;
            }
        }
    });
}