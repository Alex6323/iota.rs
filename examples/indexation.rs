// Copyright 2020 IOTA Stiftung
// SPDX-License-Identifier: Apache-2.0

use iota::{Client, Payload};

#[tokio::main]
async fn main() {
    let iota = Client::builder() // Crate a client instance builder
        .node("http://0.0.0.0:14265") // Insert the node here
        .unwrap()
        .build()
        .unwrap();

    let r = iota
        .send()
        .indexation()
        .index("Hello".to_string())
        .data("Tangle".to_string().as_bytes().to_vec())
        .post()
        .await
        .unwrap();

    println!("MessageId {}", r);

    let fetched_messages = iota.get_message().index(&"Hello").await.unwrap();

    println!("{:#?}", fetched_messages);

    let r = iota.get_message().data(&fetched_messages[0]).await.unwrap();

    println!("{:#?}", r);
    if let Payload::Indexation(i) = r.payload().as_ref().unwrap() {
        println!(
            "Data: {}",
            String::from_utf8(i.data().to_vec()).expect("Found invalid UTF-8")
        );
    }
}
