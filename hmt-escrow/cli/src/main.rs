use clap::{
    crate_description, crate_name, crate_version, value_t, value_t_or_exit, App, AppSettings, Arg,
    SubCommand,
};
use solana_clap_utils::{
    input_parsers::pubkey_of,
    input_validators::{is_keypair, is_parsable, is_pubkey, is_url},
    keypair::signer_from_path,
};
use solana_client::{
    rpc_client::RpcClient,
};
use solana_program::{instruction::Instruction, program_pack::Pack, pubkey::Pubkey};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    native_token::*,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use hmt_escrow::{
    self,
    state::Escrow,
    processor::Processor as EscrowProcessor,
    instruction::initialize as initialize_escrow,
};
use spl_token::{
    self, instruction::initialize_account,
    state::Account as TokenAccount,
};
use std::process::exit;

struct Config {
    rpc_client: RpcClient,
    verbose: bool,
    owner: Box<dyn Signer>,
    fee_payer: Box<dyn Signer>,
    commitment_config: CommitmentConfig,
}

type Error = Box<dyn std::error::Error>;
type CommandResult = Result<Option<Transaction>, Error>;

macro_rules! unique_signers {
    ($vec:ident) => {
        $vec.sort_by_key(|l| l.pubkey());
        $vec.dedup();
    };
}

fn check_fee_payer_balance(config: &Config, required_balance: u64) -> Result<(), Error> {
    let balance = config.rpc_client.get_balance(&config.fee_payer.pubkey())?;
    if balance < required_balance {
        Err(format!(
            "Fee payer, {}, has insufficient balance: {} required, {} available",
            config.fee_payer.pubkey(),
            lamports_to_sol(required_balance),
            lamports_to_sol(balance)
        )
        .into())
    } else {
        Ok(())
    }
}

fn command_create_escrow(config: &Config, mint: &Pubkey, launcher: &Option<Pubkey>, canceler: &Option<Pubkey>, canceler_token: &Option<Pubkey>, duration: u64) -> CommandResult {
    
    let escrow_token_account = Keypair::new();
    println!(
        "Creating escrow token account {}",
        escrow_token_account.pubkey()
    );

    let escrow_account = Keypair::new();
    
    let token_account_balance = config
        .rpc_client
        .get_minimum_balance_for_rent_exemption(TokenAccount::LEN)?;
    let escrow_account_balance = config
        .rpc_client
        .get_minimum_balance_for_rent_exemption(Escrow::LEN)?;
    let total_rent_free_balances =
        token_account_balance + escrow_account_balance;

    // Calculate withdraw authority used for minting pool tokens
    let (authority, _) = EscrowProcessor::find_authority_bump_seed(
        &hmt_escrow::id(),
        &escrow_account.pubkey(),
    );

    if config.verbose {
        println!("Escrow authority {}", authority);
    }

    let mut instructions: Vec<Instruction> = vec![
        // Account for the escrow tokens
        system_instruction::create_account(
            &config.fee_payer.pubkey(),
            &escrow_token_account.pubkey(),
            token_account_balance,
            TokenAccount::LEN as u64,
            &spl_token::id(),
        ),
        // Account for the escrow
        system_instruction::create_account(
            &config.fee_payer.pubkey(),
            &escrow_account.pubkey(),
            escrow_account_balance,
            Escrow::LEN as u64,
            &hmt_escrow::id(),
        ),
        // Initialize escrow token account
        initialize_account(
            &spl_token::id(),
            &escrow_token_account.pubkey(),
            mint,
            &authority,
        )?,
    ];

    let mut signers = vec![
        config.fee_payer.as_ref(),
        &escrow_token_account,
        &escrow_account,
    ];

    // Unwrap optionals
    let launcher: Pubkey = launcher.unwrap_or(config.owner.pubkey());
    let canceler: Pubkey = canceler.unwrap_or(config.owner.pubkey());

    let canceler_token_account = Keypair::new();
    let canceler_token: Pubkey = match canceler_token {
        Some(value) => *value,
        None => {
            println!(
                "Creating canceler token account {}",
                canceler_token_account.pubkey()
            );

            instructions.extend(vec![
                // Account for the canceler tokens
                system_instruction::create_account(
                    &config.fee_payer.pubkey(),
                    &canceler_token_account.pubkey(),
                    token_account_balance,
                    TokenAccount::LEN as u64,
                    &spl_token::id(),
                ),
                // Initialize canceler token account
                initialize_account(
                    &spl_token::id(),
                    &canceler_token_account.pubkey(),
                    mint,
                    &canceler,
                )?,
            ]);

            signers.push(&canceler_token_account);

            canceler_token_account.pubkey()
        }
    };

    println!("Creating escrow {}", escrow_account.pubkey());
    instructions.extend(vec![
        // Initialize escrow account
        initialize_escrow(
            &hmt_escrow::id(),
            &escrow_account.pubkey(),
            mint,
            &escrow_token_account.pubkey(),
            &launcher,
            &canceler,
            &canceler_token,
            duration,
        )?,
    ]);

    let mut transaction = Transaction::new_with_payer(
        &instructions,
        Some(&config.fee_payer.pubkey()),
    );

    let (recent_blockhash, fee_calculator) = config.rpc_client.get_recent_blockhash()?;
    check_fee_payer_balance(
        config,
        total_rent_free_balances + fee_calculator.calculate_fee(&transaction.message()),
    )?;
    unique_signers!(signers);
    transaction.sign(&signers, recent_blockhash);
    Ok(Some(transaction))
}

fn main() {
    let matches = App::new(crate_name!())
        .about(crate_description!())
        .version(crate_version!())
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .arg({
            let arg = Arg::with_name("config_file")
                .short("C")
                .long("config")
                .value_name("PATH")
                .takes_value(true)
                .global(true)
                .help("Configuration file to use");
            if let Some(ref config_file) = *solana_cli_config::CONFIG_FILE {
                arg.default_value(&config_file)
            } else {
                arg
            }
        })
        .arg(
            Arg::with_name("verbose")
                .long("verbose")
                .short("v")
                .takes_value(false)
                .global(true)
                .help("Show additional information"),
        )
        .arg(
            Arg::with_name("json_rpc_url")
                .long("url")
                .value_name("URL")
                .takes_value(true)
                .validator(is_url)
                .help("JSON RPC URL for the cluster.  Default from the configuration file."),
        )
        .arg(
            Arg::with_name("owner")
                .long("owner")
                .value_name("KEYPAIR")
                .validator(is_keypair)
                .takes_value(true)
                .help(
                    "Specify the stake pool or stake account owner. \
                     This may be a keypair file, the ASK keyword. \
                     Defaults to the client keypair.",
                ),
        )
        .arg(
            Arg::with_name("fee_payer")
                .long("fee-payer")
                .value_name("KEYPAIR")
                .validator(is_keypair)
                .takes_value(true)
                .help(
                    "Specify the fee-payer account. \
                     This may be a keypair file, the ASK keyword. \
                     Defaults to the client keypair.",
                ),
        )
        .subcommand(SubCommand::with_name("create").about("Create a new escrow")
            .arg(
                Arg::with_name("mint")
                    .long("mint")
                    .validator(is_pubkey)
                    .value_name("ADDRESS")
                    .takes_value(true)
                    .required(true)
                    .help("Mint address for the token managed by this escrow"),
            )
            .arg(
                Arg::with_name("launcher")
                    .long("launcher")
                    .validator(is_pubkey)
                    .value_name("ADDRESS")
                    .takes_value(true)
                    .help("Account which can manage the escrow [default: --owner]"),
            )
            .arg(
                Arg::with_name("canceler")
                    .long("canceler")
                    .validator(is_pubkey)
                    .value_name("ADDRESS")
                    .takes_value(true)
                    .help("Account which is able to cancel this escrow [default: --owner]"),
            )
            .arg(
                Arg::with_name("canceler_token")
                    .long("canceler-receiver")
                    .validator(is_pubkey)
                    .value_name("ADDRESS")
                    .takes_value(true)
                    .help("Token account which can receive tokens specified by the --mint parameter [default: new token account owned by the --canceler]"),
            )
            .arg(
                Arg::with_name("duration")
                    .long("duration")
                    .short("d")
                    .validator(is_parsable::<u64>)
                    .value_name("SECONDS")
                    .takes_value(true)
                    .required(true)
                    .help("Escrow duration in seconds, once this time passes escrow contract is no longer operational"),
            )
        )
        .get_matches();

    let mut wallet_manager = None;
    let config = {
        let cli_config = if let Some(config_file) = matches.value_of("config_file") {
            solana_cli_config::Config::load(config_file).unwrap_or_default()
        } else {
            solana_cli_config::Config::default()
        };
        let json_rpc_url = value_t!(matches, "json_rpc_url", String)
            .unwrap_or_else(|_| cli_config.json_rpc_url.clone());

        let owner = signer_from_path(
            &matches,
            &cli_config.keypair_path,
            "owner",
            &mut wallet_manager,
        )
        .unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            exit(1);
        });
        let fee_payer = signer_from_path(
            &matches,
            &cli_config.keypair_path,
            "fee_payer",
            &mut wallet_manager,
        )
        .unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            exit(1);
        });
        let verbose = matches.is_present("verbose");

        Config {
            rpc_client: RpcClient::new(json_rpc_url),
            verbose,
            owner,
            fee_payer,
            commitment_config: CommitmentConfig::single(),
        }
    };

    solana_logger::setup_with_default("solana=info");

    let _ = match matches.subcommand() {
        ("create", Some(arg_matches)) => {
            let mint: Pubkey = pubkey_of(arg_matches, "mint").unwrap();
            let launcher: Option<Pubkey> = pubkey_of(arg_matches, "launcher");
            let canceler: Option<Pubkey> = pubkey_of(arg_matches, "canceler");
            let canceler_token: Option<Pubkey> = pubkey_of(arg_matches, "canceler_token");
            let duration = value_t_or_exit!(arg_matches, "duration", u64);
            command_create_escrow(
                &config,
                &mint,
                &launcher,
                &canceler,
                &canceler_token,
                duration,
            )
        }
        _ => unreachable!(),
    }
    .and_then(|transaction| {
        if let Some(transaction) = transaction {
            // TODO: Upgrade to solana-client 1.3 and
            // `send_and_confirm_transaction_with_spinner_and_commitment()` with single
            // confirmation by default for better UX
            let signature = config
                .rpc_client
                .send_and_confirm_transaction_with_spinner_and_commitment(
                    &transaction,
                    config.commitment_config,
                )?;
            println!("Signature: {}", signature);
        }
        Ok(())
    })
    .map_err(|err| {
        eprintln!("{}", err);
        exit(1);
    });
}
