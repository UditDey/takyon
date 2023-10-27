use takyon::time::sleep_secs;

pub fn main() {
    takyon::init().unwrap();

    let num = takyon::run(async {
        let task_1 = takyon::spawn(async {
            sleep_secs(3).await;
            9
        });

        sleep_secs(1).await;
        println!("lol");

        let task_2 = takyon::spawn(async { task_1.await });

        task_2.await
    });

    println!("{num}");
}