use takyon::{
    time::sleep_secs,
    net::TcpListener
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

        let sock = TcpListener::bind("127.0.0.1:5000").unwrap();

        loop {
            let (_stream, src_addr) = sock.accept().await.unwrap();
            println!("Got connection from {:?}", src_addr);
        }
    });
}