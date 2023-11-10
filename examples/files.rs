use takyon::fs::{File, OpenOptions};

pub fn main() {
    takyon::init().unwrap();

    takyon::run(async {
        let opts = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true);

        let path = "/home/uditd/Desktop/Projects/takyon/test_file";//std::path::PathBuf::from("/home/uditd/Desktop/Projects/takyon/test_file");
        let file = File::open(&path, &opts).await.unwrap();
    });
}