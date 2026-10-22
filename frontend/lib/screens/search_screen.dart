import 'dart:async';
import 'package:flutter/material.dart';
import '../models/conversation.dart';
import '../state/app_state.dart';
import '../theme/app_theme.dart';

class SearchMatch {
  final SyncConversation conversation;
  final String matchingSnippet;
  final bool matchedTitle;

  SearchMatch({
    required this.conversation,
    required this.matchingSnippet,
    required this.matchedTitle,
  });
}

/// Dedicated chat-history search interface inspired by ChatGPT Android.
class SearchScreen extends StatefulWidget {
  final AppState appState;
  final VoidCallback onConversationSelected;

  const SearchScreen({
    super.key,
    required this.appState,
    required this.onConversationSelected,
  });

  @override
  State<SearchScreen> createState() => _SearchScreenState();
}

class _SearchScreenState extends State<SearchScreen> {
  final TextEditingController _searchController = TextEditingController();
  final FocusNode _focusNode = FocusNode();
  Timer? _debounceTimer;
  String _currentQuery = '';
  bool _isSearching = false;
  List<SearchMatch> _results = [];

  @override
  void initState() {
    super.initState();
    _searchController.addListener(_onSearchInputChanged);
  }

  void _onSearchInputChanged() {
    final query = _searchController.text.trim();
    if (query == _currentQuery) return;

    _debounceTimer?.cancel();
    _debounceTimer = Timer(const Duration(milliseconds: 250), () {
      _performSearch(query);
    });
  }

  void _performSearch(String query) {
    if (!mounted) return;

    if (query.isEmpty) {
      setState(() {
        _currentQuery = '';
        _isSearching = false;
        _results = [];
      });
      return;
    }

    setState(() {
      _currentQuery = query;
      _isSearching = true;
    });

    final qLower = query.toLowerCase();
    final List<SearchMatch> matches = [];

    for (final conv in widget.appState.recentConversations) {
      bool added = false;

      // 1. Check title
      if (conv.title.toLowerCase().contains(qLower)) {
        matches.add(
          SearchMatch(
            conversation: conv,
            matchingSnippet: conv.title,
            matchedTitle: true,
          ),
        );
        added = true;
      }

      // 2. Check messages
      if (!added) {
        for (final msg in conv.messages) {
          final contentLower = msg.content.toLowerCase();
          final matchIdx = contentLower.indexOf(qLower);
          if (matchIdx != -1) {
            final start = (matchIdx - 30).clamp(0, msg.content.length);
            final end = (matchIdx + query.length + 40).clamp(0, msg.content.length);
            final snippet = '${start > 0 ? "..." : ""}${msg.content.substring(start, end)}${end < msg.content.length ? "..." : ""}';

            matches.add(
              SearchMatch(
                conversation: conv,
                matchingSnippet: snippet,
                matchedTitle: false,
              ),
            );
            break;
          }
        }
      }
    }

    setState(() {
      _isSearching = false;
      _results = matches;
    });
  }

  void _clearSearch() {
    _searchController.clear();
    _performSearch('');
  }

  @override
  void dispose() {
    _debounceTimer?.cancel();
    _searchController.removeListener(_onSearchInputChanged);
    _searchController.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.darkBg,
      appBar: AppBar(
        backgroundColor: AppTheme.darkBg,
        elevation: 0,
        leading: IconButton(
          icon: const Icon(Icons.arrow_back, color: AppTheme.textPrimaryDark),
          onPressed: () => Navigator.pop(context),
        ),
        title: TextField(
          controller: _searchController,
          focusNode: _focusNode,
          autofocus: true,
          style: const TextStyle(color: AppTheme.textPrimaryDark, fontSize: 16),
          decoration: InputDecoration(
            hintText: 'Search chats...',
            hintStyle: const TextStyle(color: AppTheme.textMutedDark, fontSize: 15),
            border: InputBorder.none,
            enabledBorder: InputBorder.none,
            focusedBorder: InputBorder.none,
            fillColor: Colors.transparent,
            contentPadding: EdgeInsets.zero,
            suffixIcon: _searchController.text.isNotEmpty
                ? IconButton(
                    icon: const Icon(Icons.close, size: 18, color: AppTheme.textSecondaryDark),
                    onPressed: _clearSearch,
                  )
                : null,
          ),
        ),
      ),
      body: SafeArea(
        child: Column(
          children: [
            const Divider(color: AppTheme.darkDivider),
            Expanded(child: _buildBody()),
            // Honest limitation footnote
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
              color: AppTheme.darkSurface,
              child: const Row(
                children: [
                  Icon(Icons.info_outline, size: 14, color: AppTheme.textMutedDark),
                  SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      'Client-side search across conversations loaded from Google Drive.',
                      style: TextStyle(fontSize: 11, color: AppTheme.textMutedDark),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildBody() {
    if (_isSearching) {
      return const Center(
        child: CircularProgressIndicator(
          strokeWidth: 2,
          color: AppTheme.textSecondaryDark,
        ),
      );
    }

    if (_currentQuery.isEmpty) {
      return Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(Icons.search, size: 48, color: AppTheme.textMutedDark.withOpacity(0.4)),
            const SizedBox(height: 12),
            const Text(
              'Search your conversations',
              style: TextStyle(fontSize: 15, color: AppTheme.textSecondaryDark),
            ),
            const SizedBox(height: 4),
            const Text(
              'Type keywords to search titles and message content',
              style: TextStyle(fontSize: 12, color: AppTheme.textMutedDark),
            ),
          ],
        ),
      );
    }

    if (_results.isEmpty) {
      return Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const Icon(Icons.search_off, size: 48, color: AppTheme.textMutedDark),
            const SizedBox(height: 12),
            Text(
              'No results for "$_currentQuery"',
              style: const TextStyle(fontSize: 15, color: AppTheme.textPrimaryDark),
            ),
            const SizedBox(height: 4),
            const Text(
              'Try a different search term',
              style: TextStyle(fontSize: 12, color: AppTheme.textMutedDark),
            ),
          ],
        ),
      );
    }

    return ListView.separated(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      itemCount: _results.length,
      separatorBuilder: (_, _) => const Divider(color: AppTheme.darkDivider),
      itemBuilder: (context, index) {
        final match = _results[index];
        return ListTile(
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
          contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
          title: Text(
            match.conversation.title,
            style: const TextStyle(
              fontSize: 14,
              fontWeight: FontWeight.w600,
              color: AppTheme.textPrimaryDark,
            ),
          ),
          subtitle: Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              match.matchingSnippet,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                fontSize: 12,
                color: AppTheme.textSecondaryDark,
                height: 1.3,
              ),
            ),
          ),
          trailing: const Icon(Icons.chevron_right, size: 16, color: AppTheme.textMutedDark),
          onTap: () {
            widget.appState.selectConversation(match.conversation.id);
            Navigator.pop(context);
            widget.onConversationSelected();
          },
        );
      },
    );
  }
}
