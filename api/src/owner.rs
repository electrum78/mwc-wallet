// Copyright 2018 The Grin Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Owner API External Definition

use chrono::prelude::*;
use uuid::Uuid;

use crate::config::WalletConfig;
use crate::core::core::Transaction;
use crate::core::global;
use crate::impls::create_sender;
use crate::keychain::{Identifier, Keychain};
use crate::libwallet::api_impl::owner;
use crate::libwallet::{
	AcctPathMapping, Error, ErrorKind, InitTxArgs, IssueInvoiceTxArgs, NodeClient,
	NodeHeightResult, OutputCommitMapping, Slate, TxLogEntry, WalletInfo, WalletInst,
	WalletLCProvider,
};
use crate::util::secp::key::SecretKey;
use crate::util::{from_hex, static_secp_instance, LoggingConfig, Mutex, ZeroingString};
use std::sync::Arc;

/// Main interface into all wallet API functions.
/// Wallet APIs are split into two seperate blocks of functionality
/// called the ['Owner'](struct.Owner.html) and ['Foreign'](struct.Foreign.html) APIs
///
/// * The 'Owner' API is intended to expose methods that are to be
/// used by the wallet owner only. It is vital that this API is not
/// exposed to anyone other than the owner of the wallet (i.e. the
/// person with access to the seed and password.
///
/// Methods in both APIs are intended to be 'single use', that is to say each
/// method will 'open' the wallet (load the keychain with its master seed), perform
/// its operation, then 'close' the wallet (unloading references to the keychain and master
/// seed).

pub struct Owner<'a, L, C, K>
where
	L: WalletLCProvider<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	/// contain all methods to manage the wallet
	pub wallet_inst: Arc<Mutex<Box<dyn WalletInst<'a, L, C, K>>>>,
	/// Flag to normalize some output during testing. Can mostly be ignored.
	pub doctest_mode: bool,
	/// Share ECDH key
	pub shared_key: Arc<Mutex<Option<SecretKey>>>,
}

impl<'a, L, C, K> Owner<'a, L, C, K>
where
	L: WalletLCProvider<'a, C, K>,
	C: NodeClient,
	K: Keychain,
{
	/// Create a new API instance with the given wallet instance. All subsequent
	/// API calls will operate on this instance of the wallet.
	///
	/// Each method will call the [`WalletBackend`](../grin_wallet_libwallet/types/trait.WalletBackend.html)'s
	/// [`open_with_credentials`](../grin_wallet_libwallet/types/trait.WalletBackend.html#tymethod.open_with_credentials)
	/// (initialising a keychain with the master seed,) perform its operation, then close the keychain
	/// with a call to [`close`](../grin_wallet_libwallet/types/trait.WalletBackend.html#tymethod.close)
	///
	/// # Arguments
	/// * `wallet_in` - A reference-counted mutex containing an implementation of the
	/// [`WalletBackend`](../grin_wallet_libwallet/types/trait.WalletBackend.html) trait.
	///
	/// # Returns
	/// * An instance of the OwnerApi holding a reference to the provided wallet
	///
	/// # Example
	/// ```
	/// use grin_wallet_util::grin_keychain as keychain;
	/// use grin_wallet_util::grin_util as util;
	/// use grin_wallet_api as api;
	/// use grin_wallet_config as config;
	/// use grin_wallet_impls as impls;
	/// use grin_wallet_libwallet as libwallet;
	///
	/// use keychain::ExtKeychain;
	/// use tempfile::tempdir;
	///
	/// use std::sync::Arc;
	/// use util::{Mutex, ZeroingString};
	///
	/// use api::Owner;
	/// use config::WalletConfig;
	/// use impls::{DefaultWalletImpl, DefaultLCProvider, HTTPNodeClient};
	/// use libwallet::WalletInst;
	///
	/// let mut wallet_config = WalletConfig::default();
	/// # let dir = tempdir().map_err(|e| format!("{:#?}", e)).unwrap();
	/// # let dir = dir
	/// # 	.path()
	/// # 	.to_str()
	/// # 	.ok_or("Failed to convert tmpdir path to string.".to_owned())
	/// # 	.unwrap();
	/// # wallet_config.data_file_dir = dir.to_owned();
	///
	/// // A NodeClient must first be created to handle communication between
	/// // the wallet and the node.
	/// let node_client = HTTPNodeClient::new(&wallet_config.check_node_api_http_addr, None);
	///
	/// // impls::DefaultWalletImpl is provided for convenience in instantiating the wallet
	/// // It contains the LMDBBackend, DefaultLCProvider (lifecycle) and ExtKeychain used
	/// // by the reference wallet implementation.
	/// // These traits can be replaced with alternative implementations if desired
	///
	/// let mut wallet = Box::new(DefaultWalletImpl::<'static, HTTPNodeClient>::new(node_client.clone()).unwrap())
	///		as Box<WalletInst<'static, DefaultLCProvider<HTTPNodeClient, ExtKeychain>, HTTPNodeClient, ExtKeychain>>;
	///
	/// // Wallet LifeCycle Provider provides all functions init wallet and work with seeds, etc...
	/// let lc = wallet.lc_provider().unwrap();
	///
	/// // The top level wallet directory should be set manually (in the reference implementation,
	/// // this is provided in the WalletConfig)
	/// let _ = lc.set_top_level_directory(&wallet_config.data_file_dir);
	///
	/// // Wallet must be opened with the password (TBD)
	/// let pw = ZeroingString::from("wallet_password");
	/// lc.open_wallet(None, pw, false, false);
	///
	/// // All wallet functions operate on an Arc::Mutex to allow multithreading where needed
	/// let mut wallet = Arc::new(Mutex::new(wallet));
	///
	/// let api_owner = Owner::new(wallet.clone());
	/// // .. perform wallet operations
	///
	/// ```

	pub fn new(wallet_inst: Arc<Mutex<Box<dyn WalletInst<'a, L, C, K>>>>) -> Self {
		Owner {
			wallet_inst,
			doctest_mode: false,
			shared_key: Arc::new(Mutex::new(None)),
		}
	}

	/// Returns a list of accounts stored in the wallet (i.e. mappings between
	/// user-specified labels and BIP32 derivation paths.
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	///
	/// # Returns
	/// * Result Containing:
	/// * A Vector of [`AcctPathMapping`](../grin_wallet_libwallet/types/struct.AcctPathMapping.html) data
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Remarks
	///
	/// * A wallet should always have the path with the label 'default' path defined,
	/// with path m/0/0
	/// * This method does not need to use the wallet seed or keychain.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let api_owner = Owner::new(wallet.clone());
	///
	/// let result = api_owner.accounts(None);
	///
	/// if let Ok(accts) = result {
	///		//...
	/// }
	/// ```

	pub fn accounts(
		&self,
		keychain_mask: Option<&SecretKey>,
	) -> Result<Vec<AcctPathMapping>, Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		// Test keychain mask, to keep API consistent
		let _ = w.keychain(keychain_mask)?;
		owner::accounts(&mut **w)
	}

	/// Creates a new 'account', which is a mapping of a user-specified
	/// label to a BIP32 path
	///
	/// # Arguments
	///
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `label` - A human readable label to which to map the new BIP32 Path
	///
	/// # Returns
	/// * Result Containing:
	/// * A [Keychain Identifier](../grin_keychain/struct.Identifier.html) for the new path
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Remarks
	///
	/// * Wallets should be initialised with the 'default' path mapped to `m/0/0`
	/// * Each call to this function will increment the first element of the path
	/// so the first call will create an account at `m/1/0` and the second at
	/// `m/2/0` etc. . .
	/// * The account path is used throughout as the parent key for most key-derivation
	/// operations. See [`set_active_account`](struct.Owner.html#method.set_active_account) for
	/// further details.
	///
	/// * This function does not need to use the root wallet seed or keychain.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let api_owner = Owner::new(wallet.clone());
	///
	/// let result = api_owner.create_account_path(None, "account1");
	///
	/// if let Ok(identifier) = result {
	///		//...
	/// }
	/// ```

	pub fn create_account_path(
		&self,
		keychain_mask: Option<&SecretKey>,
		label: &str,
	) -> Result<Identifier, Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::create_account_path(&mut **w, keychain_mask, label)
	}

	/// Sets the wallet's currently active account. This sets the
	/// BIP32 parent path used for most key-derivation operations.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `label` - The human readable label for the account. Accounts can be retrieved via
	/// the [`account`](struct.Owner.html#method.accounts) method
	///
	/// # Returns
	/// * Result Containing:
	/// * `Ok(())` if the path was correctly set
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Remarks
	///
	/// * Wallet parent paths are 2 path elements long, e.g. `m/0/0` is the path
	/// labelled 'default'. Keys derived from this parent path are 3 elements long,
	/// e.g. the secret keys derived from the `m/0/0` path will be  at paths `m/0/0/0`,
	/// `m/0/0/1` etc...
	///
	/// * This function does not need to use the root wallet seed or keychain.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let api_owner = Owner::new(wallet.clone());
	///
	/// let result = api_owner.create_account_path(None, "account1");
	///
	/// if let Ok(identifier) = result {
	///		// set the account active
	///		let result2 = api_owner.set_active_account(None, "account1");
	/// }
	/// ```

	pub fn set_active_account(
		&self,
		keychain_mask: Option<&SecretKey>,
		label: &str,
	) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		// Test keychain mask, to keep API consistent
		let _ = w.keychain(keychain_mask)?;
		owner::set_active_account(&mut **w, label)
	}

	/// Returns a list of outputs from the active account in the wallet.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `include_spent` - If `true`, outputs that have been marked as 'spent'
	/// in the wallet will be returned. If `false`, spent outputs will omitted
	/// from the results.
	/// * `refresh_from_node` - If true, the wallet will attempt to contact
	/// a node (via the [`NodeClient`](../grin_wallet_libwallet/types/trait.NodeClient.html)
	/// provided during wallet instantiation). If `false`, the results will
	/// contain output information that may be out-of-date (from the last time
	/// the wallet's output set was refreshed against the node).
	/// * `tx_id` - If `Some(i)`, only return the outputs associated with
	/// the transaction log entry of id `i`.
	///
	/// # Returns
	/// * `(bool, Vec<OutputCommitMapping>)` - A tuple:
	/// * The first `bool` element indicates whether the data was successfully
	/// refreshed from the node (note this may be false even if the `refresh_from_node`
	/// argument was set to `true`.
	/// * The second element contains a vector of
	/// [OutputCommitMapping](../grin_wallet_libwallet/types/struct.OutputCommitMapping.html)
	/// of which each element is a mapping between the wallet's internal
	/// [OutputData](../grin_wallet_libwallet/types/struct.Output.html)
	/// and the Output commitment as identified in the chain's UTXO set
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let api_owner = Owner::new(wallet.clone());
	/// let show_spent = false;
	/// let update_from_node = true;
	/// let tx_id = None;
	///
	/// let result = api_owner.retrieve_outputs(None, show_spent, update_from_node, tx_id);
	///
	/// if let Ok((was_updated, output_mappings)) = result {
	///		//...
	/// }
	/// ```

	pub fn retrieve_outputs(
		&self,
		keychain_mask: Option<&SecretKey>,
		include_spent: bool,
		refresh_from_node: bool,
		tx_id: Option<u32>,
	) -> Result<(bool, Vec<OutputCommitMapping>), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::retrieve_outputs(
			&mut **w,
			keychain_mask,
			include_spent,
			refresh_from_node,
			tx_id,
		)
	}

	/// Returns a list of [Transaction Log Entries](../grin_wallet_libwallet/types/struct.TxLogEntry.html)
	/// from the active account in the wallet.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `refresh_from_node` - If true, the wallet will attempt to contact
	/// a node (via the [`NodeClient`](../grin_wallet_libwallet/types/trait.NodeClient.html)
	/// provided during wallet instantiation). If `false`, the results will
	/// contain transaction information that may be out-of-date (from the last time
	/// the wallet's output set was refreshed against the node).
	/// * `tx_id` - If `Some(i)`, only return the transactions associated with
	/// the transaction log entry of id `i`.
	/// * `tx_slate_id` - If `Some(uuid)`, only return transactions associated with
	/// the given [`Slate`](../grin_wallet_libwallet/slate/struct.Slate.html) uuid.
	///
	/// # Returns
	/// * `(bool, Vec<TxLogEntry)` - A tuple:
	/// * The first `bool` element indicates whether the data was successfully
	/// refreshed from the node (note this may be false even if the `refresh_from_node`
	/// argument was set to `true`.
	/// * The second element contains the set of retrieved
	/// [TxLogEntries](../grin_wallet_libwallet/types/struct.TxLogEntry.html)
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let api_owner = Owner::new(wallet.clone());
	/// let update_from_node = true;
	/// let tx_id = None;
	/// let tx_slate_id = None;
	///
	/// // Return all TxLogEntries
	/// let result = api_owner.retrieve_txs(None, update_from_node, tx_id, tx_slate_id);
	///
	/// if let Ok((was_updated, tx_log_entries)) = result {
	///		//...
	/// }
	/// ```

	pub fn retrieve_txs(
		&self,
		keychain_mask: Option<&SecretKey>,
		refresh_from_node: bool,
		tx_id: Option<u32>,
		tx_slate_id: Option<Uuid>,
	) -> Result<(bool, Vec<TxLogEntry>), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		let mut res = owner::retrieve_txs(
			&mut **w,
			keychain_mask,
			refresh_from_node,
			tx_id,
			tx_slate_id,
		)?;
		if self.doctest_mode {
			res.1 = res
				.1
				.into_iter()
				.map(|mut t| {
					t.confirmation_ts = Some(Utc.ymd(2019, 1, 15).and_hms(16, 1, 26));
					t.creation_ts = Utc.ymd(2019, 1, 15).and_hms(16, 1, 26);
					t
				})
				.collect();
		}
		Ok(res)
	}

	/// Returns summary information from the active account in the wallet.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `refresh_from_node` - If true, the wallet will attempt to contact
	/// a node (via the [`NodeClient`](../grin_wallet_libwallet/types/trait.NodeClient.html)
	/// provided during wallet instantiation). If `false`, the results will
	/// contain transaction information that may be out-of-date (from the last time
	/// the wallet's output set was refreshed against the node).
	/// * `minimum_confirmations` - The minimum number of confirmations an output
	/// should have before it's included in the 'amount_currently_spendable' total
	///
	/// # Returns
	/// * (`bool`, [`WalletInfo`](../grin_wallet_libwallet/types/struct.WalletInfo.html)) - A tuple:
	/// * The first `bool` element indicates whether the data was successfully
	/// refreshed from the node (note this may be false even if the `refresh_from_node`
	/// argument was set to `true`.
	/// * The second element contains the Summary [`WalletInfo`](../grin_wallet_libwallet/types/struct.WalletInfo.html)
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// let update_from_node = true;
	/// let minimum_confirmations=10;
	///
	/// // Return summary info for active account
	/// let result = api_owner.retrieve_summary_info(None, update_from_node, minimum_confirmations);
	///
	/// if let Ok((was_updated, summary_info)) = result {
	///		//...
	/// }
	/// ```

	pub fn retrieve_summary_info(
		&self,
		keychain_mask: Option<&SecretKey>,
		refresh_from_node: bool,
		minimum_confirmations: u64,
	) -> Result<(bool, WalletInfo), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::retrieve_summary_info(
			&mut **w,
			keychain_mask,
			refresh_from_node,
			minimum_confirmations,
		)
	}

	/// Initiates a new transaction as the sender, creating a new
	/// [`Slate`](../grin_wallet_libwallet/slate/struct.Slate.html) object containing
	/// the sender's inputs, change outputs, and public signature data. This slate can
	/// then be sent to the recipient to continue the transaction via the
	/// [Foreign API's `receive_tx`](struct.Foreign.html#method.receive_tx) method.
	///
	/// When a transaction is created, the wallet must also lock inputs (and create unconfirmed
	/// outputs) corresponding to the transaction created in the slate, so that the wallet doesn't
	/// attempt to re-spend outputs that are already included in a transaction before the transaction
	/// is confirmed. This method also returns a function that will perform that locking, and it is
	/// up to the caller to decide the best time to call the lock function
	/// (via the [`tx_lock_outputs`](struct.Owner.html#method.tx_lock_outputs) method).
	/// If the exchange method is intended to be synchronous (such as via a direct http call,)
	/// then the lock call can wait until the response is confirmed. If it is asynchronous, (such
	/// as via file transfer,) the lock call should happen immediately (before the file is sent
	/// to the recipient).
	///
	/// If the `send_args` [`InitTxSendArgs`](../grin_wallet_libwallet/types/struct.InitTxSendArgs.html),
	/// of the [`args`](../grin_wallet_libwallet/types/struct.InitTxArgs.html), field is Some, this
	/// function will attempt to perform a synchronous send to the recipient specified in the `dest`
	/// field according to the `method` field, and will also finalize and post the transaction if
	/// the `finalize` field is set.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `args` - [`InitTxArgs`](../grin_wallet_libwallet/types/struct.InitTxArgs.html),
	/// transaction initialization arguments. See struct documentation for further detail.
	///
	/// # Returns
	/// * a result containing:
	/// * The transaction [Slate](../grin_wallet_libwallet/slate/struct.Slate.html),
	/// which can be forwarded to the recieving party by any means. Once the caller is relatively
	/// certain that the transaction has been sent to the recipient, the associated wallet
	/// transaction outputs should be locked via a call to
	/// [`tx_lock_outputs`](struct.Owner.html#method.tx_lock_outputs). This must be called before calling
	/// [`finalize_tx`](struct.Owner.html#method.finalize_tx).
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Remarks
	///
	/// * This method requires an active connection to a node, and will fail with error if a node
	/// cannot be contacted to refresh output statuses.
	/// * This method will store a partially completed transaction in the wallet's transaction log,
	/// which will be updated on the corresponding call to [`finalize_tx`](struct.Owner.html#method.finalize_tx).
	///
	/// # Example
	/// Set up as in [new](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// // Attempt to create a transaction using the 'default' account
	/// let args = InitTxArgs {
	/// 	src_acct_name: None,
	/// 	amount: 2_000_000_000,
	/// 	minimum_confirmations: 2,
	/// 	max_outputs: 500,
	/// 	num_change_outputs: 1,
	/// 	selection_strategy_is_use_all: true,
	/// 	message: Some("Have some Grins. Love, Yeastplume".to_owned()),
	/// 	..Default::default()
	/// };
	/// let result = api_owner.init_send_tx(
	/// 	None,
	/// 	args,
	/// );
	///
	/// if let Ok(slate) = result {
	/// 	// Send slate somehow
	/// 	// ...
	/// 	// Lock our outputs if we're happy the slate was (or is being) sent
	/// 	api_owner.tx_lock_outputs(None, &slate, 0);
	/// }
	/// ```

	pub fn init_send_tx(
		&self,
		keychain_mask: Option<&SecretKey>,
		args: InitTxArgs,
	) -> Result<Slate, Error> {
		let send_args = args.send_args.clone();
		let mut slate = {
			let mut w_lock = self.wallet_inst.lock();
			let w = w_lock.lc_provider()?.wallet_inst()?;
			owner::init_send_tx(&mut **w, keychain_mask, args, self.doctest_mode)?
		};
		// Helper functionality. If send arguments exist, attempt to send
		match send_args {
			Some(sa) => {
				//TODO: in case of keybase, the response might take 60s and leave the service hanging
				match sa.method.as_ref() {
					"http" | "keybase" => {}
					_ => {
						error!("unsupported payment method: {}", sa.method);
						return Err(ErrorKind::ClientCallback(
							"unsupported payment method".to_owned(),
						)
						.into());
					}
				};
				let comm_adapter = create_sender(&sa.method, &sa.dest)
					.map_err(|e| ErrorKind::GenericError(format!("{}", e)))?;
				slate = comm_adapter.send_tx(&slate)?;
				self.tx_lock_outputs(keychain_mask, &slate, 0)?;
				let slate = match sa.finalize {
					true => self.finalize_tx(keychain_mask, &slate)?,
					false => slate,
				};

				if sa.post_tx {
					self.post_tx(keychain_mask, &slate.tx, sa.fluff)?;
				}
				Ok(slate)
			}
			None => Ok(slate),
		}
	}

	/// Issues a new invoice transaction slate, essentially a `request for payment`.
	/// The slate created by this function will contain the amount, an output for the amount,
	/// as well as round 1 of singature creation complete. The slate should then be send
	/// to the payer, who should add their inputs and signature data and return the slate
	/// via the [Foreign API's `finalize_invoice_tx`](struct.Foreign.html#method.finalize_invoice_tx) method.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `args` - [`IssueInvoiceTxArgs`](../grin_wallet_libwallet/types/struct.IssueInvoiceTxArgs.html),
	/// invoice transaction initialization arguments. See struct documentation for further detail.
	///
	/// # Returns
	/// * ``Ok([`slate`](../grin_wallet_libwallet/slate/struct.Slate.html))` if successful,
	/// containing the updated slate.
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	///
	/// let args = IssueInvoiceTxArgs {
	/// 	amount: 60_000_000_000,
	/// 	..Default::default()
	/// };
	/// let result = api_owner.issue_invoice_tx(None, args);
	///
	/// if let Ok(slate) = result {
	///		// if okay, send to the payer to add their inputs
	///		// . . .
	/// }
	/// ```
	pub fn issue_invoice_tx(
		&self,
		keychain_mask: Option<&SecretKey>,
		args: IssueInvoiceTxArgs,
	) -> Result<Slate, Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::issue_invoice_tx(&mut **w, keychain_mask, args, self.doctest_mode)
	}

	/// Processes an invoice tranaction created by another party, essentially
	/// a `request for payment`. The incoming slate should contain a requested
	/// amount, an output created by the invoicer convering the amount, and
	/// part 1 of signature creation completed. This function will add inputs
	/// equalling the amount + fees, as well as perform round 1 and 2 of signature
	/// creation.
	///
	/// Callers should note that no prompting of the user will be done by this function
	/// it is up to the caller to present the request for payment to the user
	/// and verify that payment should go ahead.
	///
	/// This function also stores the final transaction in the user's wallet files for retrieval
	/// via the [`get_stored_tx`](struct.Owner.html#method.get_stored_tx) function.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `slate` - The transaction [`Slate`](../grin_wallet_libwallet/slate/struct.Slate.html). The
	/// payer should have filled in round 1 and 2.
	/// * `args` - [`InitTxArgs`](../grin_wallet_libwallet/types/struct.InitTxArgs.html),
	/// transaction initialization arguments. See struct documentation for further detail.
	///
	/// # Returns
	/// * ``Ok([`slate`](../grin_wallet_libwallet/slate/struct.Slate.html))` if successful,
	/// containing the updated slate.
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	///
	/// // . . .
	/// // The slate has been recieved from the invoicer, somehow
	/// # let slate = Slate::blank(2);
	/// let args = InitTxArgs {
	///		src_acct_name: None,
	///		amount: slate.amount,
	///		minimum_confirmations: 2,
	///		max_outputs: 500,
	///		num_change_outputs: 1,
	///		selection_strategy_is_use_all: true,
	///		..Default::default()
	///	};
	///
	/// let result = api_owner.process_invoice_tx(None, &slate, args);
	///
	/// if let Ok(slate) = result {
	///	// If result okay, send back to the invoicer
	///	// . . .
	///	}
	/// ```

	pub fn process_invoice_tx(
		&self,
		keychain_mask: Option<&SecretKey>,
		slate: &Slate,
		args: InitTxArgs,
	) -> Result<Slate, Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::process_invoice_tx(&mut **w, keychain_mask, slate, args, self.doctest_mode)
	}

	/// Locks the outputs associated with the inputs to the transaction in the given
	/// [`Slate`](../grin_wallet_libwallet/slate/struct.Slate.html),
	/// making them unavailable for use in further transactions. This function is called
	/// by the sender, (or more generally, all parties who have put inputs into the transaction,)
	/// and must be called before the corresponding call to [`finalize_tx`](struct.Owner.html#method.finalize_tx)
	/// that completes the transaction.
	///
	/// Outputs will generally remain locked until they are removed from the chain,
	/// at which point they will become `Spent`. It is commonplace for transactions not to complete
	/// for various reasons over which a particular wallet has no control. For this reason,
	/// [`cancel_tx`](struct.Owner.html#method.cancel_tx) can be used to manually unlock outputs
	/// and return them to the `Unspent` state.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `slate` - The transaction [`Slate`](../grin_wallet_libwallet/slate/struct.Slate.html). All
	/// * `participant_id` - The participant id, generally 0 for the party putting in funds, 1 for the
	/// party receiving.
	/// elements in the `input` vector of the `tx` field that are found in the wallet's currently
	/// active account will be set to status `Locked`
	///
	/// # Returns
	/// * Ok(()) if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// let args = InitTxArgs {
	/// 	src_acct_name: None,
	/// 	amount: 2_000_000_000,
	/// 	minimum_confirmations: 10,
	/// 	max_outputs: 500,
	/// 	num_change_outputs: 1,
	/// 	selection_strategy_is_use_all: true,
	/// 	message: Some("Remember to lock this when we're happy this is sent".to_owned()),
	/// 	..Default::default()
	/// };
	/// let result = api_owner.init_send_tx(
	/// 	None,
	/// 	args,
	/// );
	///
	/// if let Ok(slate) = result {
	///		// Send slate somehow
	///		// ...
	///		// Lock our outputs if we're happy the slate was (or is being) sent
	///		api_owner.tx_lock_outputs(None, &slate, 0);
	/// }
	/// ```

	pub fn tx_lock_outputs(
		&self,
		keychain_mask: Option<&SecretKey>,
		slate: &Slate,
		participant_id: usize,
	) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::tx_lock_outputs(&mut **w, keychain_mask, slate, participant_id)
	}

	/// Finalizes a transaction, after all parties
	/// have filled in both rounds of Slate generation. This step adds
	/// all participants partial signatures to create the final signature,
	/// resulting in a final transaction that is ready to post to a node.
	///
	/// Note that this function DOES NOT POST the transaction to a node
	/// for validation. This is done in separately via the
	/// [`post_tx`](struct.Owner.html#method.post_tx) function.
	///
	/// This function also stores the final transaction in the user's wallet files for retrieval
	/// via the [`get_stored_tx`](struct.Owner.html#method.get_stored_tx) function.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `slate` - The transaction [`Slate`](../grin_wallet_libwallet/slate/struct.Slate.html). All
	/// participants must have filled in both rounds, and the sender should have locked their
	/// outputs (via the [`tx_lock_outputs`](struct.Owner.html#method.tx_lock_outputs) function).
	///
	/// # Returns
	/// * ``Ok([`slate`](../grin_wallet_libwallet/slate/struct.Slate.html))` if successful,
	/// containing the new finalized slate.
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// let args = InitTxArgs {
	/// 	src_acct_name: None,
	/// 	amount: 2_000_000_000,
	/// 	minimum_confirmations: 10,
	/// 	max_outputs: 500,
	/// 	num_change_outputs: 1,
	/// 	selection_strategy_is_use_all: true,
	/// 	message: Some("Finalize this tx now".to_owned()),
	/// 	..Default::default()
	/// };
	/// let result = api_owner.init_send_tx(
	/// 	None,
	/// 	args,
	/// );
	///
	/// if let Ok(slate) = result {
	///		// Send slate somehow
	///		// ...
	///		// Lock our outputs if we're happy the slate was (or is being) sent
	///		let res = api_owner.tx_lock_outputs(None, &slate, 0);
	///		//
	///		// Retrieve slate back from recipient
	///		//
	///		let res = api_owner.finalize_tx(None, &slate);
	/// }
	/// ```

	pub fn finalize_tx(
		&self,
		keychain_mask: Option<&SecretKey>,
		slate: &Slate,
	) -> Result<Slate, Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::finalize_tx(&mut **w, keychain_mask, &slate)
	}

	/// Posts a completed transaction to the listening node for validation and inclusion in a block
	/// for mining.
	///
	/// # Arguments
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `tx` - A completed [`Transaction`](../grin_core/core/transaction/struct.Transaction.html),
	/// typically the `tx` field in the transaction [`Slate`](../grin_wallet_libwallet/slate/struct.Slate.html).
	/// * `fluff` - Instruct the node whether to use the Dandelion protocol when posting the
	/// transaction. If `true`, the node should skip the Dandelion phase and broadcast the
	/// transaction to all peers immediately. If `false`, the node will follow dandelion logic and
	/// initiate the stem phase.
	///
	/// # Returns
	/// * `Ok(())` if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// let args = InitTxArgs {
	/// 	src_acct_name: None,
	/// 	amount: 2_000_000_000,
	/// 	minimum_confirmations: 10,
	/// 	max_outputs: 500,
	/// 	num_change_outputs: 1,
	/// 	selection_strategy_is_use_all: true,
	/// 	message: Some("Post this tx".to_owned()),
	/// 	..Default::default()
	/// };
	/// let result = api_owner.init_send_tx(
	/// 	None,
	/// 	args,
	/// );
	///
	/// if let Ok(slate) = result {
	///		// Send slate somehow
	///		// ...
	///		// Lock our outputs if we're happy the slate was (or is being) sent
	///		let res = api_owner.tx_lock_outputs(None, &slate, 0);
	///		//
	///		// Retrieve slate back from recipient
	///		//
	///		let res = api_owner.finalize_tx(None, &slate);
	///		let res = api_owner.post_tx(None, &slate.tx, true);
	/// }
	/// ```

	pub fn post_tx(
		&self,
		keychain_mask: Option<&SecretKey>,
		tx: &Transaction,
		fluff: bool,
	) -> Result<(), Error> {
		let client = {
			let mut w_lock = self.wallet_inst.lock();
			let w = w_lock.lc_provider()?.wallet_inst()?;
			// Test keychain mask, to keep API consistent
			let _ = w.keychain(keychain_mask)?;
			w.w2n_client().clone()
		};
		owner::post_tx(&client, tx, fluff)
	}

	/// Cancels a transaction. This entails:
	/// * Setting the transaction status to either `TxSentCancelled` or `TxReceivedCancelled`
	/// * Deleting all change outputs or recipient outputs associated with the transaction
	/// * Setting the status of all assocatied inputs from `Locked` to `Spent` so they can be
	/// used in new transactions.
	///
	/// Transactions can be cancelled by transaction log id or slate id (call with either set to
	/// Some, not both)
	///
	/// # Arguments
	///
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `tx_id` - If present, cancel by the [`TxLogEntry`](../grin_wallet_libwallet/types/struct.TxLogEntry.html) id
	/// for the transaction.
	///
	/// * `tx_slate_id` - If present, cancel by the Slate id.
	///
	/// # Returns
	/// * `Ok(())` if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// let args = InitTxArgs {
	/// 	src_acct_name: None,
	/// 	amount: 2_000_000_000,
	/// 	minimum_confirmations: 10,
	/// 	max_outputs: 500,
	/// 	num_change_outputs: 1,
	/// 	selection_strategy_is_use_all: true,
	/// 	message: Some("Cancel this tx".to_owned()),
	/// 	..Default::default()
	/// };
	/// let result = api_owner.init_send_tx(
	/// 	None,
	/// 	args,
	/// );
	///
	/// if let Ok(slate) = result {
	///		// Send slate somehow
	///		// ...
	///		// Lock our outputs if we're happy the slate was (or is being) sent
	///		let res = api_owner.tx_lock_outputs(None, &slate, 0);
	///		//
	///		// We didn't get the slate back, or something else went wrong
	///		//
	///		let res = api_owner.cancel_tx(None, None, Some(slate.id.clone()));
	/// }
	/// ```

	pub fn cancel_tx(
		&self,
		keychain_mask: Option<&SecretKey>,
		tx_id: Option<u32>,
		tx_slate_id: Option<Uuid>,
	) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::cancel_tx(&mut **w, keychain_mask, tx_id, tx_slate_id)
	}

	/// Retrieves the stored transaction associated with a TxLogEntry. Can be used even after the
	/// transaction has completed.
	///
	/// # Arguments
	///
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `tx_log_entry` - A [`TxLogEntry`](../grin_wallet_libwallet/types/struct.TxLogEntry.html)
	///
	/// # Returns
	/// * Ok with the stored  [`Transaction`](../grin_core/core/transaction/struct.Transaction.html)
	/// if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let api_owner = Owner::new(wallet.clone());
	/// let update_from_node = true;
	/// let tx_id = None;
	/// let tx_slate_id = None;
	///
	/// // Return all TxLogEntries
	/// let result = api_owner.retrieve_txs(None, update_from_node, tx_id, tx_slate_id);
	///
	/// if let Ok((was_updated, tx_log_entries)) = result {
	///		let stored_tx = api_owner.get_stored_tx(None, &tx_log_entries[0]).unwrap();
	///		//...
	/// }
	/// ```

	// TODO: Should be accepting an id, not an entire entry struct
	pub fn get_stored_tx(
		&self,
		keychain_mask: Option<&SecretKey>,
		tx_log_entry: &TxLogEntry,
	) -> Result<Option<Transaction>, Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		// Test keychain mask, to keep API consistent
		let _ = w.keychain(keychain_mask)?;
		owner::get_stored_tx(&**w, tx_log_entry)
	}

	/// Verifies all messages in the slate match their public keys.
	///
	/// The optional messages themselves are part of the `participant_data` field within the slate.
	/// Messages are signed with the same key used to sign for the paricipant's inputs, and can thus be
	/// verified with the public key found in the `public_blind_excess` field. This function is a
	/// simple helper to returns whether all signatures in the participant data match their public
	/// keys.
	///
	/// # Arguments
	///
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `slate` - The transaction [`Slate`](../grin_wallet_libwallet/slate/struct.Slate.html).
	///
	/// # Returns
	/// * `Ok(())` if successful and the signatures validate
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// let args = InitTxArgs {
	/// 	src_acct_name: None,
	/// 	amount: 2_000_000_000,
	/// 	minimum_confirmations: 10,
	/// 	max_outputs: 500,
	/// 	num_change_outputs: 1,
	/// 	selection_strategy_is_use_all: true,
	/// 	message: Some("Just verify messages".to_owned()),
	/// 	..Default::default()
	/// };
	/// let result = api_owner.init_send_tx(
	/// 	None,
	/// 	args,
	/// );
	///
	/// if let Ok(slate) = result {
	///		// Send slate somehow
	///		// ...
	///		// Lock our outputs if we're happy the slate was (or is being) sent
	///		let res = api_owner.tx_lock_outputs(None, &slate, 0);
	///		//
	///		// Retrieve slate back from recipient
	///		//
	///		let res = api_owner.verify_slate_messages(None, &slate);
	/// }
	/// ```
	pub fn verify_slate_messages(
		&self,
		keychain_mask: Option<&SecretKey>,
		slate: &Slate,
	) -> Result<(), Error> {
		{
			let mut w_lock = self.wallet_inst.lock();
			let w = w_lock.lc_provider()?.wallet_inst()?;
			// Test keychain mask, to keep API consistent
			let _ = w.keychain(keychain_mask)?;
		}
		owner::verify_slate_messages(slate)
	}

	/// Scans the entire UTXO set from the node, creating outputs for each scanned
	/// output that matches the wallet's master seed. This function is intended to be called as part
	/// of a recovery process (either from BIP32 phrase or backup seed files,) and will error if the
	/// wallet is non-empty, i.e. contains any outputs at all.
	///
	/// This operation scans the entire chain, and is expected to be time intensive. It is imperative
	/// that no other processes should be trying to use the wallet at the same time this function is
	/// running.
	///
	/// A single [TxLogEntry](../grin_wallet_libwallet/types/struct.TxLogEntry.html) is created for
	/// all non-coinbase outputs discovered and restored during this process. A separate entry
	/// is created for each coinbase output.
	///
	/// # Arguments
	///
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	///
	/// # Returns
	/// * `Ok(())` if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.

	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// let result = api_owner.restore(None);
	///
	/// if let Ok(_) = result {
	///		// Wallet outputs should be consistent with what's on chain
	///		// ...
	/// }
	/// ```
	pub fn restore(&self, keychain_mask: Option<&SecretKey>) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		let res = owner::restore(&mut **w, keychain_mask);
		res
	}

	/// Scans the entire UTXO set from the node, identify which outputs belong to the given wallet
	/// update the wallet state to be consistent with what's currently in the UTXO set.
	///
	/// This function can be used to repair wallet state, particularly by restoring outputs that may
	/// be missing if the wallet owner has cancelled transactions locally that were then successfully
	/// posted to the chain.
	///
	/// This operation scans the entire chain, and is expected to be time intensive. It is imperative
	/// that no other processes should be trying to use the wallet at the same time this function is
	/// running.
	///
	/// When an output is found that doesn't exist in the wallet, a corresponding
	/// [TxLogEntry](../grin_wallet_libwallet/types/struct.TxLogEntry.html) is created.
	///
	/// # Arguments
	///
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	/// * `delete_unconfirmed` - if `false`, the check_repair process will be non-destructive, and
	/// mostly limited to restoring missing outputs. It will leave unconfirmed transaction logs entries
	/// and unconfirmed outputs intact. If `true`, the process will unlock all locked outputs,
	/// restore all missing outputs, and mark any outputs that have been marked 'Spent' but are still
	/// in the UTXO set as 'Unspent' (as can happen during a fork). It will also attempt to cancel any
	/// transaction log entries associated with any locked outputs or outputs incorrectly marked 'Spent'.
	/// Note this completely removes all outstanding transactions, so users should be very aware what
	/// will happen if this flag is set. Note that if transactions/outputs are removed that later
	/// confirm on the chain, another call to this function will restore them.
	///
	/// # Returns
	/// * `Ok(())` if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.

	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let mut api_owner = Owner::new(wallet.clone());
	/// let result = api_owner.check_repair(
	/// 	None,
	/// 	false,
	/// );
	///
	/// if let Ok(_) = result {
	///		// Wallet outputs should be consistent with what's on chain
	///		// ...
	/// }
	/// ```

	pub fn check_repair(
		&self,
		keychain_mask: Option<&SecretKey>,
		delete_unconfirmed: bool,
	) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		owner::check_repair(&mut **w, keychain_mask, delete_unconfirmed)
	}

	/// Retrieves the last known height known by the wallet. This is determined as follows:
	/// * If the wallet can successfully contact its configured node, the reported node
	/// height is returned, and the `updated_from_node` field in the response is `true`
	/// * If the wallet cannot contact the node, this function returns the maximum height
	/// of all outputs contained within the wallet, and the `updated_from_node` fields
	/// in the response is set to false.
	///
	/// Clients should generally ensure the `updated_from_node` field is returned as
	/// `true` before assuming the height for any operation.
	///
	/// # Arguments
	///
	/// * `keychain_mask` - Wallet secret mask to XOR against the stored wallet seed before using, if
	/// being used.
	///
	/// # Returns
	/// * Ok with a  [`NodeHeightResult`](../grin_wallet_libwallet/types/struct.NodeHeightResult.html)
	/// if successful. If the height result was obtained from the configured node,
	/// `updated_from_node` will be set to `true`
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let api_owner = Owner::new(wallet.clone());
	/// let result = api_owner.node_height(None);
	///
	/// if let Ok(node_height_result) = result {
	///		if node_height_result.updated_from_node {
	///			//we can assume node_height_result.height is relatively safe to use
	///
	///		}
	///		//...
	/// }
	/// ```

	pub fn node_height(
		&self,
		keychain_mask: Option<&SecretKey>,
	) -> Result<NodeHeightResult, Error> {
		let mut w_lock = self.wallet_inst.lock();
		let w = w_lock.lc_provider()?.wallet_inst()?;
		// Test keychain mask, to keep API consistent
		let _ = w.keychain(keychain_mask)?;
		owner::node_height(&mut **w, keychain_mask)
	}

	// LIFECYCLE FUNCTIONS

	/// Retrieve the top-level directory for the wallet. This directory should contain the
	/// `mwc-wallet.toml` file and the `wallet_data` directory that contains the wallet
	/// seed + data files. Future versions of the wallet API will support multiple wallets
	/// within the top level directory.
	///
	/// The top level directory defaults to (in order of precedence):
	///
	/// 1) The current directory, from which `grin-wallet` or the main process was run, if it
	/// contains a `mwc-wallet.toml` file.
	/// 2) ~/.grin/<chaintype>/ otherwise
	///
	/// # Arguments
	///
	/// * None
	///
	/// # Returns
	/// * Ok with a String value representing the full path to the top level wallet dierctory
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let api_owner = Owner::new(wallet.clone());
	/// let result = api_owner.get_top_level_directory();
	///
	/// if let Ok(dir) = result {
	///		println!("Top level directory is: {}", dir);
	///		//...
	/// }
	/// ```

	pub fn get_top_level_directory(&self) -> Result<String, Error> {
		let mut w_lock = self.wallet_inst.lock();
		let lc = w_lock.lc_provider()?;
		if self.doctest_mode {
			Ok("/doctest/dir".to_owned())
		} else {
			lc.get_top_level_directory()
		}
	}

	/// Set the top-level directory for the wallet. This directory can be empty, and will be created
	/// during a subsequent calls to [`create_config`](struct.Owner.html#method.create_config)
	///
	/// Set [`get_top_level_directory`](struct.Owner.html#method.get_top_level_directory) for a
	/// description of the top level directory and default paths.
	///
	/// # Arguments
	///
	/// * `dir`: The new top-level directory path (either relative to current directory or
	/// absolute.
	///
	/// # Returns
	/// * Ok if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// let dir = "path/to/wallet/dir";
	///
	/// # let dir = tempdir().map_err(|e| format!("{:#?}", e)).unwrap();
	/// # let dir = dir
	/// # 	.path()
	/// # 	.to_str()
	/// # 	.ok_or("Failed to convert tmpdir path to string.".to_owned())
	/// # 	.unwrap();
	///
	/// let api_owner = Owner::new(wallet.clone());
	/// let result = api_owner.set_top_level_directory(dir);
	///
	/// if let Ok(dir) = result {
	///		//...
	/// }
	/// ```

	pub fn set_top_level_directory(&self, dir: &str) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let lc = w_lock.lc_provider()?;
		lc.set_top_level_directory(dir)
	}

	/// Create a `mwc-wallet.toml` configuration file in the top-level directory for the
	/// specified chain type.
	/// A custom [`WalletConfig`](../grin_wallet_config/types/struct.WalletConfig.html)
	/// and/or grin `LoggingConfig` may optionally be provided, otherwise defaults will be used.
	///
	/// Paths in the configuration file will be updated to reflect the top level directory, so
	/// path-related values in the optional configuration structs will be ignored.
	///
	/// # Arguments
	///
	/// * `chain_type`: The chain type to use in creation of the configuration file. This can be
	///     * `AutomatedTesting`
	///     * `UserTesting`
	///     * `Floonet`
	///     * `Mainnet`
	///
	/// # Returns
	/// * Ok if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// use grin_core::global::ChainTypes;
	///
	/// let dir = "path/to/wallet/dir";
	///
	/// # let dir = tempdir().map_err(|e| format!("{:#?}", e)).unwrap();
	/// # let dir = dir
	/// # 	.path()
	/// # 	.to_str()
	/// # 	.ok_or("Failed to convert tmpdir path to string.".to_owned())
	/// # 	.unwrap();
	///
	/// let api_owner = Owner::new(wallet.clone());
	/// let _ = api_owner.set_top_level_directory(dir);
	///
	/// let result = api_owner.create_config(&ChainTypes::Mainnet, None, None);
	///
	/// if let Ok(_) = result {
	///		//...
	/// }
	/// ```

	pub fn create_config(
		&self,
		chain_type: &global::ChainTypes,
		wallet_config: Option<WalletConfig>,
		logging_config: Option<LoggingConfig>,
	) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let lc = w_lock.lc_provider()?;
		lc.create_config(chain_type, "mwc-wallet.toml", wallet_config, logging_config)
	}

	/// Creates a new wallet seed and empty wallet database in the `wallet_data` directory of
	/// the top level directory.
	///
	/// Paths in the configuration file will be updated to reflect the top level directory, so
	/// path-related values in the optional configuration structs will be ignored.
	///
	/// The wallet files must not already exist, and ~The `mwc-wallet.toml` file must exist
	/// in the top level directory (can be created via a call to
	/// [`create_config`](struct.Owner.html#method.create_config))
	///
	/// # Arguments
	///
	/// * `name`: Reserved for future use, use `None` for the time being.
	/// * `mnemonic`: If present, restore the wallet seed from the given mnemonic instead of creating
	/// a new random seed.
	/// * `mnemonic_length`: Desired length of mnemonic in bytes (16 or 32, either 12 or 24 words).
	/// Use 0 if mnemonic isn't being used.
	/// * `password`: The password used to encrypt/decrypt the `wallet.seed` file
	///
	/// # Returns
	/// * Ok if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// use grin_core::global::ChainTypes;
	///
	///	// note that the WalletInst struct does not necessarily need to contain an
	///	// instantiated wallet
	///
	/// let dir = "path/to/wallet/dir";
	///
	/// # let dir = tempdir().map_err(|e| format!("{:#?}", e)).unwrap();
	/// # let dir = dir
	/// # 	.path()
	/// # 	.to_str()
	/// # 	.ok_or("Failed to convert tmpdir path to string.".to_owned())
	/// # 	.unwrap();
	/// let api_owner = Owner::new(wallet.clone());
	/// let _ = api_owner.set_top_level_directory(dir);
	///
	/// // Create configuration
	/// let result = api_owner.create_config(&ChainTypes::Mainnet, None, None);
	///
	///	// create new wallet wirh random seed
	///	let pw = ZeroingString::from("my_password");
	/// let result = api_owner.create_wallet(None, None, 0, pw);
	///
	/// if let Ok(r) = result {
	///		//...
	/// }
	/// ```

	pub fn create_wallet(
		&self,
		name: Option<&str>,
		mnemonic: Option<ZeroingString>,
		mnemonic_length: u32,
		password: ZeroingString,
	) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let lc = w_lock.lc_provider()?;
		lc.create_wallet(
			name,
			mnemonic,
			mnemonic_length as usize,
			password,
			self.doctest_mode,
		)
	}

	/// `Opens` a wallet, populating the internal keychain with the encrypted seed, and optionally
	/// returning a `keychain_mask` token to the caller to provide in all future calls.
	/// If using a mask, the seed will be stored in-memory XORed against the `keychain_mask`, and
	/// will not be useable if the mask is not provided.
	///
	/// # Arguments
	///
	/// * `name`: Reserved for future use, use `None` for the time being.
	/// * `password`: The password to use to open the wallet
	/// a new random seed.
	/// * `use_mask`: Whether to create and return a mask which much be provided in all future
	/// API calls.
	///
	/// # Returns
	/// * Ok if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// use grin_core::global::ChainTypes;
	///
	///	// note that the WalletInst struct does not necessarily need to contain an
	///	// instantiated wallet
	/// let dir = "path/to/wallet/dir";
	///
	/// # let dir = tempdir().map_err(|e| format!("{:#?}", e)).unwrap();
	/// # let dir = dir
	/// # 	.path()
	/// # 	.to_str()
	/// # 	.ok_or("Failed to convert tmpdir path to string.".to_owned())
	/// # 	.unwrap();
	/// let api_owner = Owner::new(wallet.clone());
	/// let _ = api_owner.set_top_level_directory(dir);
	///
	/// // Create configuration
	/// let result = api_owner.create_config(&ChainTypes::Mainnet, None, None);
	///
	///	// create new wallet wirh random seed
	///	let pw = ZeroingString::from("my_password");
	/// let _ = api_owner.create_wallet(None, None, 0, pw.clone());
	///
	/// let result = api_owner.open_wallet(None, pw, true);
	///
	/// if let Ok(m) = result {
	///		// use this mask in all subsequent calls
	///		let mask = m;
	/// }
	/// ```

	pub fn open_wallet(
		&self,
		name: Option<&str>,
		password: ZeroingString,
		use_mask: bool,
	) -> Result<Option<SecretKey>, Error> {
		// just return a representative string for doctest mode
		if self.doctest_mode {
			let secp_inst = static_secp_instance();
			let secp = secp_inst.lock();
			return Ok(Some(SecretKey::from_slice(
				&secp,
				&from_hex(
					"d096b3cb75986b3b13f80b8f5243a9edf0af4c74ac37578c5a12cfb5b59b1868".to_owned(),
				)
				.unwrap(),
			)?));
		}
		let mut w_lock = self.wallet_inst.lock();
		let lc = w_lock.lc_provider()?;
		lc.open_wallet(name, password, use_mask, self.doctest_mode)
	}

	/// `Close` a wallet, removing the master seed from memory.
	///
	/// # Arguments
	///
	/// * `name`: Reserved for future use, use `None` for the time being.
	///
	/// # Returns
	/// * Ok if successful
	/// * or [`libwallet::Error`](../grin_wallet_libwallet/struct.Error.html) if an error is encountered.
	///
	/// # Example
	/// Set up as in [`new`](struct.Owner.html#method.new) method above.
	/// ```
	/// # grin_wallet_api::doctest_helper_setup_doc_env!(wallet, wallet_config);
	///
	/// use grin_core::global::ChainTypes;
	///
	///	// Set up as above
	/// # let api_owner = Owner::new(wallet.clone());
	///
	/// let res = api_owner.close_wallet(None);
	///
	/// if let Ok(_) = res {
	///		// ...
	/// }
	/// ```

	pub fn close_wallet(&self, name: Option<&str>) -> Result<(), Error> {
		let mut w_lock = self.wallet_inst.lock();
		let lc = w_lock.lc_provider()?;
		lc.close_wallet(name)
	}
}

#[doc(hidden)]
#[macro_export]
macro_rules! doctest_helper_setup_doc_env {
	($wallet:ident, $wallet_config:ident) => {
		use grin_wallet_api as api;
		use grin_wallet_config as config;
		use grin_wallet_impls as impls;
		use grin_wallet_libwallet as libwallet;
		use grin_wallet_util::grin_core;
		use grin_wallet_util::grin_keychain as keychain;
		use grin_wallet_util::grin_util as util;

		use keychain::ExtKeychain;
		use tempfile::tempdir;

		use std::sync::Arc;
		use util::{Mutex, ZeroingString};

		use api::{Foreign, Owner};
		use config::WalletConfig;
		use impls::{DefaultLCProvider, DefaultWalletImpl, HTTPNodeClient};
		use libwallet::{BlockFees, InitTxArgs, IssueInvoiceTxArgs, Slate, WalletInst};

		let dir = tempdir().map_err(|e| format!("{:#?}", e)).unwrap();
		let dir = dir
			.path()
			.to_str()
			.ok_or("Failed to convert tmpdir path to string.".to_owned())
			.unwrap();
		let mut wallet_config = WalletConfig::default();
		wallet_config.data_file_dir = dir.to_owned();
		let pw = ZeroingString::from("");

		let node_client = HTTPNodeClient::new(&wallet_config.check_node_api_http_addr, None);
		let mut wallet = Box::new(
			DefaultWalletImpl::<'static, HTTPNodeClient>::new(node_client.clone()).unwrap(),
			)
			as Box<
				WalletInst<
					'static,
					DefaultLCProvider<HTTPNodeClient, ExtKeychain>,
					HTTPNodeClient,
					ExtKeychain,
				>,
				>;
		let lc = wallet.lc_provider().unwrap();
		let _ = lc.set_top_level_directory(&wallet_config.data_file_dir);
		lc.open_wallet(None, pw, false, false);
		let mut $wallet = Arc::new(Mutex::new(wallet));
	};
}
