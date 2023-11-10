use takyon::fs::{File, OpenOptions};

pub fn main() {
    takyon::init().unwrap();

    takyon::run(async {
        let opts = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true);

        let file = File::open("test_file", &opts).await.unwrap();

        file.write(b"hello").await.unwrap();
    });
}