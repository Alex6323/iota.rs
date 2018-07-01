mod address;

pub use self::address::*;

use crate::utils::checksum;
use crate::utils::constants;
use crate::utils::converter;
use crate::utils::input_validator;
use crate::utils::right_pad;
use crate::utils::signing;

use crate::model::bundle::{self, Bundle};
use crate::model::transaction::Transaction;
use crate::model::transfer::Transfer;
use crate::pow::curl::{Curl, STATE_LENGTH};
use crate::pow::kerl::Kerl;
use crate::pow::{Sponge, HASH_LENGTH};

use crate::iri_api;

use chrono::prelude::*;
use failure::Error;

pub fn get_key(seed: &str, index: usize, security: usize) -> String {
    converter::trytes(&signing::key(
        &converter::trits_from_string_with_length(seed, 81 * security),
        index,
        security,
    ))
}

pub fn get_digest(seed: &str, security: usize, index: usize) -> String {
    let key = signing::key(
        &converter::trits_from_string_with_length(&seed, 243),
        index,
        security,
    );
    converter::trytes(&signing::digests(&key))
}

pub fn validate_address(address: &str, digests: &[String]) -> bool {
    let mut kerl = Kerl::default();
    for digest in digests {
        kerl.absorb(&converter::trits_from_string(digest));
    }
    let mut address_trits = [0; HASH_LENGTH];
    kerl.squeeze(&mut address_trits);
    converter::trytes(&address_trits) == address
}

pub fn initiate_transfer(
    security_sum: usize,
    balance: Option<i64>,
    address: &str,
    remainder_address: &str,
    transfers: &mut [Transfer],
) -> Result<Vec<Transaction>, Error> {
    for transfer in transfers.iter_mut() {
        *transfer.address_mut() = checksum::remove_checksum(transfer.address());
    }
    ensure!(
        input_validator::is_address(address),
        "Invalid address [{}]",
        address
    );
    ensure!(
        input_validator::is_address(remainder_address),
        "Invalid address [{}]",
        remainder_address
    );
    ensure!(
        input_validator::is_transfers_collection_valid(transfers),
        "Invalid transfers [{:?}]",
        transfers
    );

    let mut bundle = Bundle::default();
    let mut total_value: i64 = 0;
    let mut signature_fragments: Vec<String> = Vec::new();
    let mut tag: String = String::new();

    for transfer in transfers.iter_mut() {
        let mut signature_message_length = 1;
        if transfer.message().len() > constants::MESSAGE_LENGTH {
            signature_message_length += (transfer.message().len() as f64
                / constants::MESSAGE_LENGTH as f64)
                .floor() as usize;
            let mut msg_copy = transfer.message().to_string();
            while !msg_copy.is_empty() {
                let mut fragment: String =
                    msg_copy.chars().take(constants::MESSAGE_LENGTH).collect();
                msg_copy = msg_copy
                    .chars()
                    .skip(constants::MESSAGE_LENGTH)
                    .take(msg_copy.len())
                    .collect();
                right_pad(&mut fragment, constants::MESSAGE_LENGTH, '9');
                signature_fragments.push(fragment);
            }
        } else {
            let mut fragment: String = transfer
                .message()
                .chars()
                .take(constants::MESSAGE_LENGTH)
                .collect();
            right_pad(&mut fragment, constants::MESSAGE_LENGTH, '9');
            signature_fragments.push(fragment);
        }
        tag = transfer.tag().unwrap_or_default();
        right_pad(&mut tag, constants::TAG_LENGTH, '9');
        bundle.add_entry(
            signature_message_length,
            transfer.address(),
            *transfer.value() as i64,
            &tag,
            Utc::now().timestamp(),
        );
        total_value += *transfer.value() as i64;
    }
    if total_value != 0 {
        let create_bundle = |total_balance: i64| {
            if total_balance > 0 {
                let to_subtract = 0 - total_balance;
                bundle.add_entry(
                    security_sum,
                    address,
                    to_subtract,
                    &tag,
                    Utc::now().timestamp(),
                );
            }
            ensure!(total_balance >= total_value, "Not enough balance.");
            if total_balance > total_value {
                let remainder = total_balance - total_value;
                bundle.add_entry(
                    1,
                    remainder_address,
                    remainder,
                    &tag,
                    Utc::now().timestamp(),
                );
            }
            bundle.finalize(Some(Curl::default()));
            bundle.add_trytes(&signature_fragments);
            Ok(bundle)
        };
        return Ok(if let Some(b) = balance {
            create_bundle(b)
        } else {
            let resp =
                iri_api::get_balances("https://field.carriota.com", &[address.to_string()], 100)?;
            create_bundle(resp.balances().unwrap()[0].parse()?)
        }?.transactions()
            .to_vec());
    }

    Err(format_err!(
        "Invalid value transfer: the transfer does not require a signature."
    ))
}

pub fn add_signature(bundle_to_sign: &mut Bundle, input_address: &str, key: &str) {
    let security = key.len() / constants::MESSAGE_LENGTH;
    let key = converter::trits_from_string(key);
    let mut num_signed_transactions = 0;

    for i in 0..bundle_to_sign.transactions().len() {
        let address = bundle_to_sign.transactions()[i]
            .address()
            .unwrap_or_default();
        if address == input_address {
            if input_validator::is_nine_trytes(
                &bundle_to_sign.transactions()[i]
                    .signature_fragments()
                    .unwrap_or_default(),
            ) {
                num_signed_transactions += 1;
            } else {
                let bundle_hash = bundle_to_sign.transactions()[i]
                    .bundle()
                    .unwrap_or_default();
                let first_fragment = key[0..6561].to_vec();
                let mut normalized_bundle_fragments = [[0; 27]; 3];
                let normalized_bundle_hash = bundle::normalized_bundle(&bundle_hash);

                for (k, fragment) in normalized_bundle_fragments.iter_mut().enumerate().take(3) {
                    fragment.copy_from_slice(&normalized_bundle_hash[k * 27..(k + 1) * 27]);
                }

                let first_bundle_fragment =
                    normalized_bundle_fragments[num_signed_transactions % 3];
                let first_signed_fragment =
                    signing::signature_fragment(&first_bundle_fragment, &first_fragment);

                *bundle_to_sign.transactions_mut()[i].signature_fragments_mut() =
                    Some(converter::trytes(&first_signed_fragment));

                for j in 1..security {
                    let next_fragment = key[j * 6561..(j + 1) * 6561].to_vec();
                    let next_bundle_fragment =
                        normalized_bundle_fragments[(num_signed_transactions + j) % 3];
                    let next_signed_fragment =
                        signing::signature_fragment(&next_bundle_fragment, &next_fragment);
                    *bundle_to_sign.transactions_mut()[i + j].signature_fragments_mut() =
                        Some(converter::trytes(&next_signed_fragment));
                }
                break;
            }
        }
    }
}

pub fn add_address_digest(digest_trytes: &str, curl_state_trytes: &str) -> String {
    let offset = digest_trytes.len() * 3;
    let digest = converter::trits_from_string_with_length(digest_trytes, offset);
    let mut curl_state = vec![0; offset];
    if !curl_state_trytes.is_empty() {
        curl_state.copy_from_slice(&converter::trits_from_string_with_length(
            curl_state_trytes,
            offset,
        ));
    }
    let mut curl = Curl::default();
    curl.state_mut()
        .copy_from_slice(&curl_state[0..STATE_LENGTH]);
    curl.absorb(&digest);
    converter::trytes(curl.state())
}

pub fn finalize_address(curl_state_trytes: &str) -> String {
    let curl_state = converter::trits_from_string(curl_state_trytes);
    let mut curl = Curl::default();
    curl.state_mut().copy_from_slice(&curl_state);
    let mut address_trits = [0; HASH_LENGTH];
    curl.squeeze(&mut address_trits);
    converter::trytes(&address_trits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iota_api;
    use crate::model::bundle::Bundle;
    use crate::model::transfer::Transfer;
    use crate::multisig::Address;

    const TEST_SEED1: &str = "ABCDFG";
    const TEST_SEED2: &str = "FDSAG";
    const REMAINDER_ADDRESS: &str = "NZRALDYNVGJWUVLKDWFKJVNYLWQGCWYCURJIIZRLJIKSAIVZSGEYKTZRDBGJLOA9AWYJQB9IPWRAKUC9FBDRZJZXZG";
    const RECEIVE_ADDRESS: &str =
        "ZGHXPZYDKXPEOSQTAQOIXEEI9K9YKFKCWKYYTYAUWXK9QZAVMJXWAIZABOXHHNNBJIEBEUQRTBWGLYMTX";
    const TEST_TAG: &str = "JOTASPAM9999999999999999999";

    #[test]
    fn test_basic_multi_sig() {
        let mut address_builder = Address::default();
        let digests = vec![get_digest(TEST_SEED1, 3, 0), get_digest(TEST_SEED2, 3, 0)];
        address_builder.absorb(&digests);

        let multi_sig_address = address_builder.finalize(None);

        assert!(
            validate_address(&multi_sig_address, &digests),
            "Not a valid address"
        );

        let mut tmp_transfer = Transfer::default();
        *tmp_transfer.address_mut() = RECEIVE_ADDRESS.to_string();
        *tmp_transfer.value_mut() = 999;
        *tmp_transfer.message_mut() = "".to_string();
        *tmp_transfer.tag_mut() = Some(TEST_TAG.to_string());
        let mut transfers = vec![tmp_transfer];

        let transactions = iota_api::initiate_transfer(
            6,
            &multi_sig_address,
            REMAINDER_ADDRESS,
            &mut transfers,
            true,
        ).unwrap();
        println!("{:?}", transactions);
        let mut bundle = Bundle::new(&transactions, transactions.len());
        add_signature(&mut bundle, &multi_sig_address, &get_key(TEST_SEED1, 0, 3));
        add_signature(&mut bundle, &multi_sig_address, &get_key(TEST_SEED2, 0, 3));
        let is_valid_sig = signing::validate_bundle_signatures(&bundle, &multi_sig_address);
        assert!(is_valid_sig, "not valid");
    }
//https://pow2.iota.community
    #[test]
    fn test_basic_multi_sig2() {
        println!("{:?}", iri_api::attach_to_tangle("https://trinity.iota.fm", "RWPF9JABLCVCD9XZEOOD9DPCJOTVFILURFQYDITPDBBXVZVXXAMQNQAEIVVYQGPSIBAIKGWDTMKSA9999", "ROYAHQAWSLEFQWBVJBPMWJDIBNYH9RZAOAPPNYSPKYUJGHZFJRTXHYTCWWJNKFGSYWXQE9KOIQRFZ9999", 14, &["BYSWEAUTWXHXZ9YBZISEK9LUHWGMHXCGEVNZHRLUWQFCUSDXZHOFHWHL9MQPVJXXZLIXPXPXF9KYEREFSKCPKYIIKPZVLHUTDFQKKVVBBN9ATTLPCNPJDWDEVIYYLGPZGCWXOBDXMLJC9VO9QXTTBLAXTTBFUAROYEGQIVB9MJWJKXJMCUPTWAUGFZBTZCSJVRBGMYXTVBDDS9MYUJCPZ9YDWWQNIPUAIJXXSNLKUBSCOIJPCLEFPOXFJREXQCUVUMKSDOVQGGHRNILCO9GNCLWFM9APMNMWYASHXQAYBEXF9QRIHIBHYEJOYHRQJAOKAQ9AJJFQ9WEIWIJOTZATIBOXQLBMIJU9PCGBLVDDVFP9CFFSXTDUXMEGOOFXWRTLFGV9XXMYWEMGQEEEDBTIJ9OJOXFAPFQXCDAXOUDMLVYRMRLUDBETOLRJQAEDDLNVIRQJUBZBO9CCFDHIX9MSQCWYAXJVWHCUPTRSXJDESISQPRKZAFKFRULCGVRSBLVFOPEYLEE99JD9SEBALQINPDAZHFAB9RNBH9AZWIJOTLBZVIEJIAYGMC9AZGNFWGRSWAXTYSXVROVNKCOQQIWGPNQZKHUNODGYADPYLZZZUQRTJRTODOUKAOITNOMWNGHJBBA99QUMBHRENGBHTH9KHUAOXBVIVDVYYZMSEYSJWIOGGXZVRGN999EEGQMCOYVJQRIRROMPCQBLDYIGQO9AMORPYFSSUGACOJXGAQSPDY9YWRRPESNXXBDQ9OZOXVIOMLGTSWAMKMTDRSPGJKGBXQIVNRJRFRYEZ9VJDLHIKPSKMYC9YEGHFDS9SGVDHRIXBEMLFIINOHVPXIFAZCJKBHVMQZEVWCOSNWQRDYWVAIBLSCBGESJUIBWZECPUCAYAWMTQKRMCHONIPKJYYTEGZCJYCT9ABRWTJLRQXKMWY9GWZMHYZNWPXULNZAPVQLPMYQZCYNEPOCGOHBJUZLZDPIXVHLDMQYJUUBEDXXPXFLNRGIPWBRNQQZJSGSJTTYHIGGFAWJVXWL9THTPWOOHTNQWCNYOYZXALHAZXVMIZE9WMQUDCHDJMIBWKTYH9AC9AFOT9DPCADCV9ZWUTE9QNOMSZPTZDJLJZCJGHXUNBJFUBJWQUEZDMHXGBPTNSPZBR9TGSKVOHMOQSWPGFLSWNESFKSAZY9HHERAXALZCABFYPOVLAHMIHVDBGKUMDXC9WHHTIRYHZVWNXSVQUWCR9M9RAGMFEZZKZ9XEOQGOSLFQCHHOKLDSA9QCMDGCGMRYJZLBVIFOLBIJPROKMHOYTBTJIWUZWJMCTKCJKKTR9LCVYPVJI9AHGI9JOWMIWZAGMLDFJA9WU9QAMEFGABIBEZNNAL9OXSBFLOEHKDGHWFQSHMPLYFCNXAAZYJLMQDEYRGL9QKCEUEJ9LLVUOINVSZZQHCIKPAGMT9CAYIIMTTBCPKWTYHOJIIY9GYNPAJNUJ9BKYYXSV9JSPEXYMCFAIKTGNRSQGUNIYZCRT9FOWENSZQPD9ALUPYYAVICHVYELYFPUYDTWUSWNIYFXPX9MICCCOOZIWRNJIDALWGWRATGLJXNAYTNIZWQ9YTVDBOFZRKO9CFWRPAQQRXTPACOWCPRLYRYSJARRKSQPR9TCFXDVIXLP9XVL99ERRDSOHBFJDJQQGGGCZNDQ9NYCTQJWVZIAELCRBJJFDMCNZU9FIZRPGNURTXOCDSQGXTQHKHUECGWFUUYS9J9NYQ9U9P9UUP9YMZHWWWCIASCFLCMSKTELZWUGCDE9YOKVOVKTAYPHDF9ZCCQAYPJIJNGSHUIHHCOSSOOBUDOKE9CJZGYSSGNCQJVBEFTZFJ9SQUHOASKRRGBSHWKBCBWBTJHOGQ9WOMQFHWJVEG9NYX9KWBTCAIXNXHEBDIOFO9ALYMFGRICLCKKLG9FOBOX9PDWNQRGHBKHGKKRLWTBEQMCWQRLHAVYYZDIIPKVQTHYTWQMTOACXZOQCDTJTBAAUWXSGJF9PNQIJ9AJRUMUVCPWYVYVARKR9RKGOUHHNKNVGGPDDLGKPQNOYHNKAVVKCXWXOQPZNSLATUJT9AUWRMPPSWHSTTYDFAQDXOCYTZHOYYGAIM9CELMZ9AZPWB9MJXGHOKDNNSZVUDAGXTJJSSZCPZVPZBYNNTUQABSXQWZCHDQSLGK9UOHCFKBIBNETK999999999999999999999999999999999999999999999999999999999999999999999999999999999NOXDXXKUDWLOFJLIPQIBRBMGDYCPGDNLQOLQS99EQYKBIU9VHCJVIPFUYCQDNY9APGEVYLCENJIOBLWNB999999999XKBRHUD99C99999999NKZKEKWLDKMJCI9N9XQOLWEPAYWSH9999999999999999999999999KDDTGZLIPBNZKMLTOLOXQVNGLASESDQVPTXALEKRMIOHQLUHD9ELQDBQETS9QFGTYOYWLNTSKKMVJAUXSIROUICDOXKSYZTDPEDKOQENTJOWJONDEWROCEJIEWFWLUAACVSJFTMCHHXJBJRKAAPUDXXVXFWP9X9999IROUICDOXKSYZTDPEDKOQENTJOWJONDEWROCEJIEWFWLUAACVSJFTMCHHXJBJRKAAPUDXXVXFWP9X9999".to_string()]).unwrap());
    }
}