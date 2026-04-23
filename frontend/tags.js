// tags.js - Tags page functionality
document.addEventListener("DOMContentLoaded", function () {
  // DOM elements
  const searchInput = document.getElementById("search-input");
  const searchButton = document.getElementById("search-button");
  const errorMessage = document.getElementById("error-message");
  const tagsContent = document.getElementById("tags-content");
  const tagsTableContainer = document.getElementById("tags-table-container");
  const tagsTableBody = document.getElementById("tags-table-body");
  const tagsCount = document.getElementById("tags-count");
  const noTags = document.getElementById("no-tags");
  const prevPageButton = document.getElementById("prev-page");
  const nextPageButton = document.getElementById("next-page");
  const currentPageSpan = document.getElementById("current-page");
  const totalPagesSpan = document.getElementById("total-pages");

  // State
  let allTags = [];
  let filteredTags = [];
  let currentPage = 1;
  const itemsPerPage = 50;

  // API endpoint
  const API_BASE = "http://localhost:3000/api";

  // Show error message
  function showError(message) {
    errorMessage.textContent = message;
    errorMessage.style.display = "block";
    setTimeout(() => {
      errorMessage.style.display = "none";
    }, 5000);
  }

  // Fetch tags from API
  async function fetchTags() {
    try {
      const response = await fetch(`${API_BASE}/tags`);
      if (!response.ok) {
        throw new Error(`HTTP error! status: ${response.status}`);
      }
      const result = await response.json();
      return result.data || [];
    } catch (error) {
      console.error("Error fetching tags:", error);
      showError(`Failed to load tags: ${error.message}`);
      return [];
    }
  }

  // Filter tags based on search term
  function filterTags(searchTerm) {
    if (!searchTerm.trim()) {
      return allTags;
    }

    const term = searchTerm.toLowerCase();
    return allTags.filter((tag) => {
      return (
        (tag.name && tag.name.toLowerCase().includes(term)) ||
        (tag.category && tag.category.toLowerCase().includes(term))
      );
    });
  }

  // Format date from timestamp
  function formatDate(timestamp) {
    if (!timestamp) return "N/A";
    const date = new Date(timestamp * 1000);
    return date.toLocaleDateString();
  }

  // Render tags table with pagination
  function renderTags(tags) {
    // Calculate pagination
    const totalTags = tags.length;
    const totalPages = Math.ceil(totalTags / itemsPerPage);

    // Ensure current page is valid
    if (currentPage > totalPages) {
      currentPage = totalPages || 1;
    }

    // Update pagination controls
    updatePaginationControls(totalPages);

    // Get tags for current page
    const startIndex = (currentPage - 1) * itemsPerPage;
    const endIndex = startIndex + itemsPerPage;
    const pageTags = tags.slice(startIndex, endIndex);

    // Update count
    tagsCount.textContent = totalTags;

    // Clear table
    tagsTableBody.innerHTML = "";

    if (pageTags.length === 0) {
      tagsTableContainer.style.display = "none";
      noTags.style.display = "block";
      return;
    }

    // Show table and hide "no results" message
    tagsTableContainer.style.display = "block";
    noTags.style.display = "none";

    // Sort tags by name
    const sortedTags = [...pageTags].sort((a, b) => {
      return (a.name || "").localeCompare(b.name || "");
    });

    // Add rows
    sortedTags.forEach((tag) => {
      const row = document.createElement("tr");
      row.style.borderBottom = "1px solid #334155";

      // ID cell
      const idCell = document.createElement("td");
      idCell.textContent = tag.id;
      idCell.style.padding = "12px 16px";

      // Name cell
      const nameCell = document.createElement("td");
      nameCell.textContent = tag.name || "N/A";
      nameCell.style.padding = "12px 16px";

      // Category cell
      const categoryCell = document.createElement("td");
      categoryCell.textContent = tag.category || "N/A";
      categoryCell.style.padding = "12px 16px";

      // Created At cell
      const createdAtCell = document.createElement("td");
      createdAtCell.textContent = formatDate(tag.createdAt);
      createdAtCell.style.padding = "12px 16px";

      // Append cells to row
      row.appendChild(idCell);
      row.appendChild(nameCell);
      row.appendChild(categoryCell);
      row.appendChild(createdAtCell);

      tagsTableBody.appendChild(row);
    });
  }

  // Update pagination controls
  function updatePaginationControls(totalPages) {
    // Update page info
    currentPageSpan.textContent = currentPage;
    totalPagesSpan.textContent = totalPages || 1;

    // Enable/disable buttons
    prevPageButton.disabled = currentPage <= 1;
    nextPageButton.disabled = currentPage >= totalPages || totalPages === 0;

    // Show/hide pagination
    const paginationElement = document.getElementById("pagination");
    if (totalPages > 1) {
      paginationElement.style.display = "flex";
    } else {
      paginationElement.style.display = "none";
    }
  }

  // Go to previous page
  function goToPreviousPage() {
    if (currentPage > 1) {
      currentPage--;
      renderTags(filteredTags);
    }
  }

  // Go to next page
  function goToNextPage() {
    const totalPages = Math.ceil(filteredTags.length / itemsPerPage);
    if (currentPage < totalPages) {
      currentPage++;
      renderTags(filteredTags);
    }
  }

  // Perform search
  function performSearch() {
    const searchTerm = searchInput.value.trim();
    filteredTags = filterTags(searchTerm);
    currentPage = 1; // Reset to first page when searching
    renderTags(filteredTags);
  }

  // Load tags
  async function loadTags() {
    // Show loading state
    tagsContent.innerHTML = `
            <div class="loading">
                <div class="loading-spinner"></div>
                <p>Loading tags...</p>
            </div>
        `;

    // Fetch data
    const tags = await fetchTags();

    // Update state
    allTags = tags;
    filteredTags = tags;

    // Hide loading, show content
    tagsContent.style.display = "none";

    // Render data
    renderTags(filteredTags);
  }

  // Event listeners
  searchButton.addEventListener("click", performSearch);

  searchInput.addEventListener("keyup", function (event) {
    if (event.key === "Enter") {
      performSearch();
    }
  });

  prevPageButton.addEventListener("click", goToPreviousPage);
  nextPageButton.addEventListener("click", goToNextPage);

  // Initial load
  loadTags();
});
