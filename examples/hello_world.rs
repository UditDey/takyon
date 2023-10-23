use takyon::{time, Error, fs::File};

pub fn main() -> Result<(), Error> {
    takyon::run!(async |exec| {
        println!("Opening file...");

        File::create(exec, "/home/uditd/Desktop/Projects/takyon/test_file").await?;
        
        println!("Done");
        time::sleep(exec, 800).await?;

        Ok(())
    })?
}