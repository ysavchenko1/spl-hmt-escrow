#![cfg(feature = "test-bpf")]

use hmt_escrow::*;
use solana_program::{hash::Hash, program_pack::Pack, pubkey::Pubkey, system_instruction};
use solana_program_test::*;
use solana_sdk::{
    signature::{Keypair, Signer},
    transaction::Transaction,
};

fn program_test() -> ProgramTest {
    let mut pc = ProgramTest::new(
        "hmt_escrow",
        id(),
        processor!(processor::Processor::process),
    );

    // Add SPL Token program
    pc.add_program(
        "spl_token",
        spl_token::id(),
        processor!(spl_token::processor::Processor::process),
    );

    pc
}

async fn create_mint(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    recent_blockhash: &Hash,
    token_mint: &Keypair,
    owner: &Pubkey,
) {
    let rent = banks_client.get_rent().await.unwrap();
    let mint_rent = rent.minimum_balance(spl_token::state::Mint::LEN);

    let mut transaction = Transaction::new_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &token_mint.pubkey(),
                mint_rent,
                spl_token::state::Mint::LEN as u64,
                &spl_token::id(),
            ),
            spl_token::instruction::initialize_mint(
                &spl_token::id(),
                &token_mint.pubkey(),
                &owner,
                None,
                0,
            )
            .unwrap(),
        ],
        Some(&payer.pubkey()),
    );
    transaction.sign(&[payer, token_mint], *recent_blockhash);
    banks_client.process_transaction(transaction).await.unwrap();
}

async fn create_token_account(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    recent_blockhash: &Hash,
    account: &Keypair,
    token_mint: &Pubkey,
    owner: &Pubkey,
) {
    let rent = banks_client.get_rent().await.unwrap();
    let account_rent = rent.minimum_balance(spl_token::state::Account::LEN);

    let mut transaction = Transaction::new_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &account.pubkey(),
                account_rent,
                spl_token::state::Account::LEN as u64,
                &spl_token::id(),
            ),
            spl_token::instruction::initialize_account(
                &spl_token::id(),
                &account.pubkey(),
                token_mint,
                owner,
            )
            .unwrap(),
        ],
        Some(&payer.pubkey()),
    );
    transaction.sign(&[payer, account], *recent_blockhash);
    banks_client.process_transaction(transaction).await.unwrap();
}

async fn create_escrow(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    recent_blockhash: &Hash,
    escrow_account: &Keypair,
    escrow_token_account: &Keypair,
    launcher: &Pubkey,
    canceler: &Pubkey,
    canceler_token: &Keypair,
    token_mint: &Pubkey,
    duration: &u64,
) {
    let rent = banks_client.get_rent().await.unwrap();
    let account_rent = rent.minimum_balance(state::Escrow::LEN);

    let mut transaction = Transaction::new_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &escrow_account.pubkey(),
                account_rent,
                hmt_escrow::state::Escrow::LEN as u64,
                &id(),
            ),
            instruction::initialize(
                &id(),
                &escrow_account.pubkey(),
                token_mint,
                &escrow_token_account.pubkey(),
                &launcher,
                &canceler,
                &canceler_token.pubkey(),
                *duration,
            )
            .unwrap(),
        ],
        Some(&payer.pubkey()),
    );
    transaction.sign(&[payer, escrow_account], *recent_blockhash);
    banks_client.process_transaction(transaction).await.unwrap();
}
struct EscrowAccount {
    pub escrow: Keypair,
    pub token_mint: Keypair,
    pub escrow_token_account: Keypair,
    pub launcher: Keypair,
    pub canceler: Keypair,
    pub canceler_token_account: Keypair,
    pub duration: u64,
    pub withdraw_authority: Pubkey,
    pub bump_seed: u8,
}

impl EscrowAccount {
    pub fn new() -> Self {
        let escrow = Keypair::new();
        let token_mint = Keypair::new();
        let escrow_token_account = Keypair::new();
        let launcher = Keypair::new();
        let canceler = Keypair::new();
        let canceler_token_account = Keypair::new();

        //find authority bumpseed
        let (withdraw_authority, bump_seed) =
            hmt_escrow::processor::Processor::find_authority_bump_seed(&id(), &escrow.pubkey());

        Self {
            escrow,
            token_mint,
            escrow_token_account,
            launcher,
            canceler,
            canceler_token_account,
            duration: 10000 as u64,
            withdraw_authority,
            bump_seed,
        }
    }

    pub async fn initialize_escrow(
        &self,
        mut banks_client: &mut BanksClient,
        payer: &Keypair,
        recent_blockhash: &Hash,
    ) {
        create_mint(
            &mut banks_client,
            &payer,
            &recent_blockhash,
            &self.token_mint,
            &self.withdraw_authority,
        )
        .await;

        //Creating token account for escrow
        create_token_account(
            &mut banks_client,
            &payer,
            &recent_blockhash,
            &self.escrow_token_account,
            &self.token_mint.pubkey(),
            &self.withdraw_authority,
        )
        .await;

        //Creating token account for canceler
        create_token_account(
            &mut banks_client,
            &payer,
            &recent_blockhash,
            &self.canceler_token_account,
            &self.token_mint.pubkey(),
            &self.withdraw_authority,
        )
        .await;
        create_escrow(
            &mut banks_client,
            &payer,
            &recent_blockhash,
            &self.escrow,
            &self.escrow_token_account,
            &self.launcher.pubkey(),
            &self.canceler.pubkey(),
            &self.canceler_token_account,
            &self.token_mint.pubkey(),
            &self.duration,
        )
        .await;
    }
}

#[tokio::test]
async fn test_hmt_escrow_initialize() {
    let (mut banks_client, payer, recent_blockhash) = program_test().start().await;
    let escrow_account = EscrowAccount::new();
    escrow_account
        .initialize_escrow(&mut banks_client, &payer, &recent_blockhash)
        .await;

    let escrow = banks_client
        .get_account(escrow_account.escrow.pubkey())
        .await
        .expect("get_account")
        .expect("stake pool not none");

    assert_eq!(escrow.data.len(), hmt_escrow::state::Escrow::LEN);

    match state::Escrow::unpack_from_slice(escrow.data.as_slice()) {
        Ok(escrow) => {
            assert_eq!(escrow.state, state::EscrowState::Launched);
            assert_eq!(escrow.bump_seed, escrow_account.bump_seed);
            assert_eq!(escrow.token_mint, escrow_account.token_mint.pubkey());
            assert_eq!(
                escrow.token_account,
                escrow_account.escrow_token_account.pubkey()
            );
            assert_eq!(escrow.canceler, escrow_account.canceler.pubkey());
            assert_eq!(
                escrow.canceler_token_account,
                escrow_account.canceler_token_account.pubkey()
            );
        }
        Err(_) => assert!(false),
    };
}
