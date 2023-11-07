use takyon::net::TcpListener;

pub fn main() {
    takyon::init().unwrap();

    takyon::run(async {
        let sock = TcpListener::bind("127.0.0.1:5000").await.unwrap();

        loop {
            let (stream, src_addr) = sock.accept().await.unwrap();
            println!("New connection from {:?}\n", src_addr);

            takyon::spawn(async move {
                let mut buf = [0; 1024];

                loop {
                    let bytes = stream.read(&mut buf).await.unwrap();

                    if bytes == 0 {
                        println!("Address {:?} disconnected\n", src_addr);
                        break;
                    }

                    println!("Read {:?} bytes from address {:?}", bytes, src_addr);
                    println!("Data: {:02X?}\n", &buf[..bytes]);
                }
            });
        }
    });
}